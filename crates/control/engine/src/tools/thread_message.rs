//! Hand text and files to the agent of another thread, one of the same topic or one shared for writing: stored and delivered as an announcement with one target.
//!
//! A message to one thread is stored and delivered as an announcement that has one thread for its target, so that
//! a thread is reached in one way whoever reaches it. The receiving thread shows it as a message from the sending
//! thread, and its agent reads a relayed block naming that thread and the stored time. It is delivered by the settings of
//! the receiving thread's topic (`broadcast.*`): steered into a turn that runs, left until the next turn, or
//! interrupting, and an idle thread is started only where that is configured. As shipped it is no input and starts
//! no turn by itself. Each message counts against the sending thread's `broadcast.rate_per_hour`, with what it
//! announced, and is audited.
//!
//! **Which thread.** The shared tree resolves names, paths and identifiers exactly as it does for reads and
//! documents. An ambiguous write is refused with reachable candidates. A thread of the sender's own topic
//! accepts a message by policy; one in another topic needs an explicit write share. Paths are never reduced
//! to their last component or tried in another topic first.
//!
//! **References** that the message names (a shared-tree path or `id:` in a link or in backquotes, with `#n` for a
//! part) are resolved from the sending thread, and the message is written for the receiving agent: each in its own
//! terms, a link with its own path, or words that name nothing where it may not read the thing. The answer lists
//! each reference that resolved and each kept as written.
//!
//! **Files** use the shared workspace upload reader and are written into the receiving thread's directory
//! under `.orch/shared/from-<sending thread>/` by the
//! courier that writes the files of every input: as file parts of the input the message becomes, or, for a
//! message that waits for the thread's next turn, by that turn before it begins. The message names where each
//! file will be. Every message keeps its blobs, also when both threads use one environment, so later changes
//! to the sending directory do not alter what was sent.

use crate::delivery::{ORCH_DIR, safe_name};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::tools::announce::{Broadcast, Plan, Reach, Sender};
use crate::tools::definitions::{Code, FILES_MAX, Refusal, TEXT_MAX_CHARS};
use crate::tools::message::Message as MessageArguments;
use crate::tools::references::Resolution;
use crate::tools::{Answer, CallContext};
use crate::unit::Unit;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::threads::{AnnouncementCategory, AnnouncementSeverity, ContentPart};
use orch_db::audit::{Actor, Entry, Origin, append};
use orch_db::queries::storage;
use orch_db::queries::threads::{ThreadRow, tools as rows};
use orch_types::{Hash, Id, Json};
use serde_json::json;
use std::collections::HashSet;
use std::fmt::Write as _;

/// The words of a message as the receiving agent reads them.
#[must_use]
pub fn message_text(sender: &str, text: &str, at: &str) -> String {
    crate::instructions::relayed(true, sender, "message", at, text)
}

/// The directory of [`ORCH_DIR`] under which the files other threads send are written.
const SENT_DIR: &str = "shared";

/// Delivery decisions enqueue input or a next-turn notice; neither acknowledges that the harness received it.
fn sent(path: &str, delivery: &str, files: Vec<Json>, references: &Resolution) -> Answer {
    let mut answer = json!({
        "target": { "kind": "thread", "path": path },
        "status": "queued",
        "delivery": delivery,
        "files": files,
    });
    if !references.is_empty() {
        answer["references"] = references.feedback();
    }
    Answer::Done(answer)
}

/// A file a message carries: its name, its size, its content where it is copied, and where the receiving agent
/// finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Carried {
    name: String,
    size: i64,
    blob: Option<Hash>,
    path: String,
}

struct Message<'a> {
    target: &'a rows::TopicThread,
    topic: Id,
    id: Id,
    plan: &'a Plan,
    sender: &'a Sender,
    carried: &'a [Carried],
    authored: &'a str,
}

/// What a message says, with the files it carries named where the receiving agent finds them.
fn with_files(text: &str, carried: &[Carried]) -> String {
    if carried.is_empty() {
        return text.to_owned();
    }
    let mut out = text.trim_end().to_owned();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("Files:");
    for file in carried {
        let _ = write!(out, "\n- {} ({} bytes)", file.path, file.size);
    }
    out
}

/// The files a message carries as the parts of an input that the courier writes: those that were copied, each at
/// the path the message names. `files` is what the row of the message keeps.
#[must_use]
pub(crate) fn file_parts(files: &Json) -> Vec<ContentPart> {
    let Some(files) = files.as_array() else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|file| {
            let blob = file
                .get("blob")
                .cloned()
                .and_then(|value| serde_json::from_value::<Hash>(value).ok())?;
            Some(ContentPart::File {
                path: file.get("path")?.as_str()?.to_owned(),
                name: file.get("name")?.as_str()?.to_owned(),
                size: file.get("size").and_then(Json::as_i64).unwrap_or(0),
                blob: Some(blob),
            })
        })
        .collect()
}

impl Engine {
    /// The thread route of `send_message`: the call of an agent.
    pub(super) async fn send_thread_message_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
        arguments: MessageArguments,
    ) -> Result<Answer, EngineError> {
        let text = arguments.text.unwrap_or_default();
        let files = arguments.files.unwrap_or_default();
        if text.chars().count() > TEXT_MAX_CHARS || files.len() > FILES_MAX {
            return Ok(Refusal::invalid(
                "\"text\" is at most 8000 characters and \"files\" takes at most 10 entries.",
            )
            .into());
        }
        if text.trim().is_empty() && files.is_empty() {
            return Ok(
                Refusal::invalid("Give text, files, or both: there is nothing to send.").into(),
            );
        }
        let (target, target_path, target_topic) = match self
            .thread_shared_for_writing(thread, &arguments.path)
            .await?
        {
            Ok(target) => target,
            Err(refusal) => return Ok(refusal.into()),
        };
        if target.node_id == thread.node_id {
            return Ok(Refusal::invalid(
                "That is your own thread. A message goes to the agent of another thread.",
            )
            .into());
        }
        if target.archived {
            return Ok(Refusal::new(
                Code::Closed,
                format!(
                    "The thread \"{}\" is archived: nothing can be sent to it.",
                    target.name
                ),
            )
            .into());
        }
        let id = ids::derived(
            "message",
            &[
                thread.node_id.as_bytes().as_slice(),
                call.call_ref.as_bytes(),
            ],
        );
        // How much an agent may send is its own topic's setting; how it reaches the thread is the thread's topic's.
        let (own, broadcast) = {
            let mut conn = self.inner.db.acquire().await?;
            let own = Broadcast::load(&mut conn, thread.topic_id).await?;
            let theirs = if target_topic == thread.topic_id {
                own.clone()
            } else {
                Broadcast::load(&mut conn, target_topic).await?
            };
            (own, theirs)
        };
        if let Some(refusal) = self.within_hourly_rate(thread, &own, id).await? {
            return Ok(refusal.into());
        }
        if let Some(saved) = self.saved_thread_message(id, &target_path).await? {
            return Ok(saved);
        }
        let carried = match self.carry_files(thread, id, &files).await? {
            Ok(carried) => carried,
            Err(refusal) => return Ok(refusal.into()),
        };
        let authored = with_files(&text, &carried);
        // What the message names is resolved from the sender, and written as the receiving agent may read it.
        let (resolution, text) = {
            let mut conn = self.inner.db.acquire().await?;
            let resolution = self
                .resolve_references(&mut conn, thread.node_id, &text)
                .await?;
            let stored = resolution.store(&text);
            (resolution, stored)
        };
        let plan = Plan {
            category: AnnouncementCategory::Notice,
            title: String::new(),
            body: with_files(&text, &carried),
            severity: AnnouncementSeverity::Info,
            delivery: broadcast.delivery,
            fallback: broadcast.fallback,
            wake_idle: broadcast.wake_idle,
            reach: Reach::All,
        };
        let sender = Sender::Agent(Box::new(thread.clone()));
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let (finished, delivered_as) = {
            let mut unit = Unit::new(&mut tx, now);
            let delivered_as = self
                .deliver_message(
                    &mut unit,
                    thread,
                    Message {
                        target: &target,
                        topic: target_topic,
                        id,
                        plan: &plan,
                        sender: &sender,
                        carried: &carried,
                        authored: &authored,
                    },
                )
                .await?;
            let delivered_as = match delivered_as {
                Ok(delivered) => delivered,
                Err(refusal) => return Ok(refusal.into()),
            };
            (unit.finish().await?, delivered_as)
        };
        self.commit(tx, finished).await?;
        let files: Vec<Json> = carried
            .iter()
            .map(|file| json!({ "name": file.name, "path": file.path }))
            .collect();
        Ok(sent(&target_path, &delivered_as, files, &resolution))
    }

    async fn saved_thread_message(
        &self,
        id: Id,
        target: &str,
    ) -> Result<Option<Answer>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(saved) = rows::announcement(&mut conn, id).await? else {
            return Ok(None);
        };
        let counted = crate::tools::announce::delivered_counts(&mut conn, &[id])
            .await?
            .remove(&id);
        let delivered = match counted {
            Some(counts) if counts.interrupted > 0 => "interrupted",
            Some(counts) if counts.steered > 0 => "steered",
            Some(counts) if counts.queued > 0 => "queued",
            _ => "deferred",
        };
        let files: Vec<Json> = saved
            .files
            .as_array()
            .into_iter()
            .flatten()
            .map(|file| json!({ "name": file["name"], "path": file["path"] }))
            .collect();
        Ok(Some(sent(target, delivered, files, &Resolution::default())))
    }

    /// Uses the common upload reader, naming each saved file in the receiving thread's shared directory.
    /// A refusal where a file cannot be read: then nothing is sent.
    async fn carry_files(
        &self,
        from: &ThreadRow,
        message: Id,
        files: &[String],
    ) -> Result<Result<Vec<Carried>, Refusal>, EngineError> {
        let files = match self.read_workspace_files(from, files, i64::MAX).await? {
            Ok(files) => files,
            Err(refusal) => return Ok(Err(refusal)),
        };
        // Two messages may carry different files with the same name; neither replaces the other's delivery.
        let folder = format!(
            "{ORCH_DIR}/{SENT_DIR}/from-{}/{message}",
            safe_name(&from.name)
        );
        let mut taken: HashSet<String> = HashSet::new();
        let mut carried = Vec::with_capacity(files.len());
        for file in files {
            let name = safe_name(&file.name);
            let mut unique = name.clone();
            let mut count = 1;
            while !taken.insert(unique.clone()) {
                count += 1;
                unique = format!("{count}-{name}");
            }
            carried.push(Carried {
                path: format!("{folder}/{unique}"),
                name: unique,
                size: file.size,
                blob: Some(file.blob),
            });
        }
        Ok(Ok(carried))
    }

    /// Stores a message as an announcement with one thread for its target and delivers it. The answer is how it
    /// reached the thread; asked again for the same message it is how it reached it the first time.
    async fn deliver_message(
        &self,
        unit: &mut Unit<'_>,
        from: &ThreadRow,
        message: Message<'_>,
    ) -> Result<Result<String, Refusal>, EngineError> {
        let Message {
            target,
            topic,
            id,
            plan,
            sender,
            carried,
            authored,
        } = message;
        let mut locked = vec![from.node_id, target.node_id];
        locked.sort_unstable();
        rows::lock_threads(&mut *unit.conn, &locked).await?;
        unit.check_tool_source().await?;
        let fresh = self.current_message_target(unit, from, &message).await?;
        let target = match fresh {
            Ok(target) => target,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let now = unit.now();
        let files = Json::Array(
            carried
                .iter()
                .map(|file| {
                    json!({ "name": file.name, "size": file.size, "blob": file.blob, "path": file.path })
                })
                .collect(),
        );
        let made = rows::insert_referenced_announcement(
            &mut *unit.conn,
            &rows::NewAnnouncement {
                id,
                topic_id: topic,
                from_thread_id: Some(from.node_id),
                from_user_id: None,
                category: "notice",
                title: "",
                body: &plan.body,
                files: &files,
                severity: "info",
                delivery: match plan.delivery {
                    orch_api_contract::topics::AnnouncementDelivery::Interrupt => "interrupt",
                    orch_api_contract::topics::AnnouncementDelivery::Steer => "steer",
                    orch_api_contract::topics::AnnouncementDelivery::NextTurn => "next_turn",
                },
                fallback: match plan.fallback {
                    orch_api_contract::topics::AnnouncementFallback::NextTurn => "next_turn",
                    orch_api_contract::topics::AnnouncementFallback::Interrupt => "interrupt",
                },
                wake_idle: plan.wake_idle,
                targets: "thread",
                to_thread_id: Some(target.node_id),
                group_id: None,
                at: now,
            },
            authored,
        )
        .await?;
        let Some(row) = made else {
            // Made before: how it reached the thread is on the line it left there.
            let counted = crate::tools::announce::delivered_counts(&mut *unit.conn, &[id])
                .await?
                .remove(&id);
            return Ok(Ok(match counted {
                Some(counts) if counts.interrupted > 0 => "interrupted",
                Some(counts) if counts.steered > 0 => "steered",
                Some(counts) if counts.queued > 0 => "queued",
                _ => "deferred",
            }
            .to_owned()));
        };
        let hashes: Vec<Hash> = carried.iter().filter_map(|file| file.blob).collect();
        orch_db::queries::uploads::lock_blobs(unit.conn, &hashes).await?;
        storage::pin_blobs(unit.conn, &hashes).await?;
        let typed = orch_documents::references::typed_in(&row.body);
        let writings = self
            .writings_for(&mut *unit.conn, target.node_id, &typed)
            .await?;
        let mut numbered = 0;
        let mut recipient = row.clone();
        recipient.body = orch_documents::references::for_reader(
            &row.body,
            &|typed| writings.get(typed).cloned(),
            &mut numbered,
        );
        let text = message_text(
            &from.name,
            &recipient.body,
            &orch_types::Time::new(row.created_at).to_string(),
        );
        let how = self
            .deliver_into(unit, target.node_id, &recipient, sender, plan, &text, true)
            .await?
            .unwrap_or("deferred");
        let reference = json!({ "thread": { "id": from.node_id, "name": from.name } });
        self.put_announcement_item(unit, target.node_id, &row, &reference, Some(how), true)
            .await?;
        let mut entry = Entry::new(
            Actor::Agent(from.node_id),
            orch_db::audit::action!(AuditAction::ThreadSentByAgent).as_str(),
            "thread",
            Some(target.node_id),
            "An agent sent a message to another thread.",
        );
        entry.node_id = Some(target.node_id);
        entry.org_id = Some(from.org_id);
        entry.details = json!({ "announcement": row.id, "from": from.node_id });
        append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
        Ok(Ok(how.to_owned()))
    }

    async fn current_message_target(
        &self,
        unit: &mut Unit<'_>,
        from: &ThreadRow,
        message: &Message<'_>,
    ) -> Result<Result<rows::TopicThread, Refusal>, EngineError> {
        let Some(current) = orch_db::queries::threads::thread(unit.conn, from.node_id).await?
        else {
            return Ok(Err(Refusal::new(
                Code::NotFound,
                "The sending thread is gone.",
            )));
        };
        if !message.carried.is_empty()
            && !crate::tools::workspace_files::same_workspace(from, &current)
        {
            return Ok(Err(crate::tools::workspace_files::changed()));
        }
        let fresh = self
            .thread_shared_for_writing_in(
                unit.conn,
                &current,
                &format!("id:{}", message.target.node_id),
            )
            .await?;
        let (target, _, topic) = match fresh {
            Ok(target) => target,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if target.archived || topic != message.topic {
            return Ok(Err(Refusal::new(
                Code::Changed,
                orch_api_contract::messages::for_failure("workspace_file_destination_changed"),
            )));
        }
        let largest = orch_settings::get::<i64>(
            unit.conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("the upload size limit has no resolved value"))?;
        if message.carried.iter().any(|file| file.size > largest) {
            return Ok(Err(Refusal::invalid(
                orch_api_contract::messages::for_failure("workspace_file_too_large"),
            )));
        }
        Ok(Ok(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::workspace_files::relative_file as sendable;

    #[test]
    fn a_file_to_send_lies_under_the_agents_directory() {
        assert!(sendable("report.txt"));
        assert!(sendable("out/build/report.txt"));
        assert!(!sendable("/etc/passwd"));
        assert!(!sendable("../other/secret"));
        assert!(!sendable("out/../../secret"));
        assert!(!sendable(""));
    }

    #[test]
    fn a_message_names_its_files_where_the_receiving_agent_finds_them() {
        let carried = [Carried {
            name: "report.txt".into(),
            size: 12,
            blob: Some(Hash::of(b"the report\n!")),
            path: ".orch/shared/from-build/report.txt".into(),
        }];
        assert_eq!(
            with_files("Done.", &carried),
            "Done.\n\nFiles:\n- .orch/shared/from-build/report.txt (12 bytes)"
        );
        assert_eq!(with_files("Done.", &[]), "Done.");
    }

    #[test]
    fn only_the_files_that_were_copied_become_parts_the_courier_writes() {
        let hash = Hash::of(b"bytes");
        let files = json!([
            { "name": "a.txt", "size": 5, "blob": hash, "path": ".orch/shared/from-x/a.txt" },
            { "name": "b.txt", "size": 3, "blob": null, "path": "/workspace/web/b.txt" },
        ]);
        assert_eq!(
            file_parts(&files),
            vec![ContentPart::File {
                path: ".orch/shared/from-x/a.txt".into(),
                name: "a.txt".into(),
                size: 5,
                blob: Some(hash),
            }]
        );
        assert!(file_parts(&json!([])).is_empty());
    }
}
