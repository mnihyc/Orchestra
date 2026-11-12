//! Agent adapter for the existing thread lifecycle, native fork and chat moderation domains.

use super::*;
use crate::engine::lock;
use crate::tools::shared::{Asking, audit_by_agent};
use crate::tools::tree::{FoundEntry, Where};
use crate::tools::{CallContext, source};
use crate::{Engine, EngineError};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::forks::ForkRequest;
use orch_authz::{Caller, Principal, epoch};
use orch_db::queries::transaction_guard::{Scope, request_scope};
use orch_db::queries::{chats, hierarchy, threads};
use orch_db::{DbError, Tx};
use orch_types::Time;
use serde_json::json;

fn missing(permission: Permission) -> Answer {
    Refusal::new(Code::NotManageable, format!(
        "This act needs {permission} from a current giver. Nothing was changed; ask a person who holds it to share that row."
    )).into()
}

fn not_confirmed() -> Answer {
    Refusal::new(Code::NotConfirmed,
        "The confirmation is missing, expired, used, or the selection or its current access changed. Nothing was done. Call manage_shared without confirm and read the complete current plan.").into()
}

fn wrong_kind() -> Answer {
    Refusal::new(Code::WrongKind,
        "archive, unarchive and fork need one thread; delete needs one thread, documents, or a chat path with message_id. Places and access pages are managed by people.").into()
}

fn plan_error(error: crate::fork_plan::PlanError) -> Result<Answer, EngineError> {
    match error {
        crate::fork_plan::PlanError::Internal(error) => Err(error),
        crate::fork_plan::PlanError::Blocked(draft) => {
            let rows = draft.lacks.iter().flat_map(|(_, missing)| missing.iter())
                .flat_map(|missing| missing.permissions.iter().map(ToString::to_string)).collect::<Vec<_>>();
            Ok(Refusal::new(Code::NotManageable, if rows.is_empty() {
                "The ordinary fork prerequisites are not available. Nothing was changed; ask a person to review the source binding and destination.".to_owned()
            } else {
                format!("The fork's giver lacks {} for the ordinary fork prerequisites. Nothing was changed.", rows.join(", "))
            }).into())
        }
        _ => Ok(Refusal::new(Code::Changed,
            "The ordinary fork cannot proceed with this source or destination. Nothing was changed; review its current binding and try again without confirm.").into()),
    }
}

impl Engine {
    /// Returns None only for an ordinary document-tree command. Every new act uses the same
    /// bounded, single-use plan store, and holds current authority through its native transaction.
    pub(super) async fn manage_session_or_chat(
        &self,
        call: &CallContext,
        thread: &threads::ThreadRow,
        asked: &Asked,
        confirm: Option<&str>,
    ) -> Result<Option<Answer>, EngineError> {
        if !matches!(
            asked.command,
            Command::Delete | Command::Archive | Command::Unarchive | Command::Fork
        ) {
            return Ok(None);
        }
        let mut tx = request_scope(Scope::Authority, self.inner.db.begin()).await?;
        source::check(&mut tx).await?;
        let located = match self
            .locate_shared(&mut tx, thread.node_id, &asked.paths[0], true, false)
            .await?
        {
            Ok(located) => located,
            Err(refusal) => {
                return Ok(Some(if confirm.is_some() {
                    not_confirmed()
                } else {
                    refusal.into()
                }));
            }
        };
        let target = match located.found.entry {
            FoundEntry::Directory(Where::Place(id))
                if located
                    .tree
                    .place(id)
                    .is_some_and(|place| place.kind == "thread") =>
            {
                Some((id, None))
            }
            FoundEntry::File(Where::Conversation(id)) => Some((id, None)),
            FoundEntry::File(Where::NodeChat(node)) => located
                .tree
                .node_chat_of(node)
                .map(|(chat, _, _)| (chat, Some(Some(node)))),
            FoundEntry::File(Where::PeopleChat(chat))
            | FoundEntry::Directory(Where::PeopleChatDir(chat)) => Some((chat, Some(None))),
            FoundEntry::File(Where::Document(_) | Where::Version(_, _))
            | FoundEntry::Directory(Where::DocumentFolder(_) | Where::Docs(_))
                if !matches!(
                    asked.command,
                    Command::Archive | Command::Unarchive | Command::Fork
                ) && asked.message_id.is_none() =>
            {
                return Ok(None);
            }
            _ if !matches!(
                asked.command,
                Command::Archive | Command::Unarchive | Command::Fork
            ) && asked.message_id.is_none() =>
            {
                return Ok(None);
            }
            _ => None,
        };
        let Some((target, chat_node)) = target else {
            return Ok(Some(wrong_kind()));
        };
        if asked.paths.len() != 1 {
            return Ok(Some(
                Refusal::invalid("Thread and chat management take exactly one path.").into(),
            ));
        }
        let now = Time::new(self.inner.clock.now());
        let asking = located.asking;
        let reach = located.tree.reach();
        let path = located.found.path;
        if let Some(node) = chat_node {
            if asked.command != Command::Delete || asked.message_id.is_none() {
                return Ok(Some(wrong_kind()));
            }
            let Some(giver) = reach.chat_manage_giver(target, node) else {
                return Ok(Some(missing(Permission::ChatManage)));
            };
            if node.is_some_and(|node| reach.archived(node)) {
                return Ok(Some(
                    Refusal::new(
                        Code::NotManageable,
                        "This chat is archived and read-only. Nothing was changed.",
                    )
                    .into(),
                ));
            }
            let message = asked
                .message_id
                .ok_or_else(|| EngineError::unreadable("a message selection is absent"))?;
            let Some(row) = chats::message(&mut tx, target, message).await? else {
                if confirm.is_some() {
                    return Ok(Some(not_confirmed()));
                }
                return Ok(Some(
                    Refusal::new(Code::NotFound, "That message is not in this chat.").into(),
                ));
            };
            if row.kind != chats::MESSAGE || row.is_deleted() {
                if confirm.is_some() {
                    return Ok(Some(not_confirmed()));
                }
                return Ok(Some(
                    Refusal::new(
                        Code::WrongKind,
                        "Select a current message, rather than a deleted message or a system line.",
                    )
                    .into(),
                ));
            }
            let stamp =
                json!({"chat":target,"message":message,"revision":row.seq_updated,"giver":giver});
            let preview = json!({"done":false,"confirm":"00000000000000000000000000000000",
                "expires_in":self.inner.config.manage_plan_kept.as_secs(),"refused":[],
                "affects":[{"path":path,"id":message,"kind":"chat_message","action":"delete","revision":row.seq_updated,
                    "attachments_removed":true,"recoverable":false}]});
            if let Some(answer) = self.confirm_shared(call, asked, confirm, stamp, preview) {
                return Ok(Some(answer));
            }
            let caller = Caller::session(giver).with_admin_mode(false);
            let principal = Principal::load(&mut tx, caller, now).await?;
            let origin = orch_db::audit::Origin::default();
            let acting = orch_chat::Acting {
                principal: &principal,
                now: now.get(),
                origin: &origin,
            };
            let mut loaded =
                orch_chat::chats::load(&mut tx, &acting, target, orch_chat::chats::Mode::Read)
                    .await
                    .map_err(EngineError::unreadable)?;
            source::check(&mut tx).await?;
            let gone = orch_chat::messages::delete_by_agent(
                &mut tx,
                &acting,
                &mut loaded,
                message,
                call.thread,
            )
            .await
            .map_err(EngineError::unreadable)?;
            let answer = json!({"done":true,"results":[{"path":path,"kind":"chat_message","action":"delete","id":message,"revision":gone.seq_updated}]});
            return self.finish_shared(tx, call, answer).await.map(Some);
        }
        if !matches!(
            asked.command,
            Command::Delete | Command::Archive | Command::Unarchive | Command::Fork
        ) || asked.message_id.is_some()
        {
            return Ok(Some(wrong_kind()));
        }
        self.lock_archive_in(&mut tx, target).await?;
        let Some(selected) = threads::thread(&mut tx, target).await? else {
            return Ok(Some(wrong_kind()));
        };
        let needed = match asked.command {
            Command::Fork => Permission::ThreadFork,
            Command::Archive | Command::Unarchive => Permission::ThreadArchive,
            _ => Permission::ThreadManage,
        };
        let Some(mut giver) = reach.thread_act_giver(target, selected.side_of.is_some(), needed)
        else {
            return Ok(Some(missing(needed)));
        };
        if reach.archived(target) && !matches!(asked.command, Command::Unarchive | Command::Delete)
        {
            return Ok(Some(
                Refusal::new(
                    Code::NotManageable,
                    "The thread is archived and read-only. Unarchive it before this act.",
                )
                .into(),
            ));
        }
        let ancestors = hierarchy::with_ancestors(&mut tx, &[target]).await?;
        let parent_archived = ancestors
            .iter()
            .any(|row| row.id != target && row.archived_at.is_some());
        let mut draft = None;
        let mut fork_stamp = Json::Null;
        if asked.command == Command::Fork {
            let destination = if let Some(to) = &asked.to {
                let found = match self
                    .locate_shared(&mut tx, thread.node_id, to, false, false)
                    .await?
                {
                    Ok(found) => found,
                    Err(refusal) => return Ok(Some(refusal.into())),
                };
                match found.found.entry {
                    FoundEntry::Directory(Where::Place(id))
                        if found
                            .tree
                            .place(id)
                            .is_some_and(|place| place.kind == "topic") =>
                    {
                        Some(id)
                    }
                    _ => {
                        return Ok(Some(
                            Refusal::new(
                                Code::WrongKind,
                                "A fork destination must be an existing topic.",
                            )
                            .into(),
                        ));
                    }
                }
            } else {
                None
            };
            let request: ForkRequest = serde_json::from_value(
                json!({"result":"new","topic_id":destination,"name":asked.name,
                "agent_access":{"where":"nothing","parts":{}}}),
            )?;
            let mut refused = None;
            for candidate in reach.thread_act_givers(target, selected.side_of.is_some(), needed) {
                let caller = Caller::session(candidate).with_admin_mode(false);
                match self
                    .draft_in(
                        &mut tx,
                        &caller,
                        target,
                        &request,
                        crate::fork_plan::Reach::Seen,
                        None,
                        None,
                    )
                    .await
                {
                    Ok(planned) if planned.is_clear() => {
                        giver = candidate;
                        fork_stamp = serde_json::to_value(&planned.plan)?;
                        if let Some(fields) = fork_stamp.as_object_mut() {
                            fields.remove("expires_at");
                        }
                        draft = Some(planned);
                        break;
                    }
                    Ok(planned) => {
                        refused = Some(crate::fork_plan::PlanError::Blocked(Box::new(planned)))
                    }
                    Err(crate::fork_plan::PlanError::Internal(error)) => return Err(error),
                    Err(error) => refused = Some(error),
                }
            }
            if draft.is_none() {
                return match refused {
                    Some(error) => plan_error(error).map(Some),
                    None => Ok(Some(missing(needed))),
                };
            }
        }
        let set = hierarchy::deletion_set(&mut tx, target, confirm.is_some()).await?;
        let native_snapshot = orch_db::queries::archive_plans::snapshot(&mut tx, target).await?;
        let work = orch_db::queries::archive_plans::work(&mut tx, target).await?;
        let mut cleanup = Vec::new();
        if asked.command == Command::Delete {
            for environment in
                orch_db::queries::lifecycle::environments_under(&mut tx, target).await?
            {
                let bindings = orch_db::queries::lifecycle::bindings(&mut tx, environment).await?;
                cleanup.push(json!({"environment":environment,"bindings":bindings}));
            }
        }
        let stamp = json!({"target":target,"giver":giver,"set":set,"archived":selected.archived_at,"native":native_snapshot,"cleanup":cleanup,
            "execution":selected.execution_revision,"queue":selected.queue_revision,"turn":selected.current_turn_id,"fork":fork_stamp});
        let mut affected = vec![
            json!({"path":path,"id":target,"kind":"thread","action":asked.command.word(),
            "settles_running_work":matches!(asked.command,Command::Archive|Command::Delete),
            "read_only_after":parent_archived || asked.command==Command::Archive,
            "recoverable":asked.command==Command::Delete}),
        ];
        if asked.command == Command::Delete {
            let until = now
                .get()
                .checked_add_signed(
                    Duration::from_std(self.inner.config.soft_delete).unwrap_or(Duration::MAX),
                )
                .unwrap_or(DateTime::<Utc>::MAX_UTC);
            affected[0]["until"] = json!(until);
            for (kind, id, revision) in &set {
                if *id != target {
                    affected.push(json!({"path":format!("id:{id}"),"id":id,"kind":kind,
                        "action":"delete","revision":revision,"until":until}));
                }
            }
        }
        if matches!(asked.command, Command::Archive | Command::Delete) {
            for row in work {
                let work = json!({"turns":row.turns,"inputs":row.inputs,"approvals":row.approvals,
                    "leases":row.leases,"session":row.session,"links":row.links});
                if let Some(entry) = affected
                    .iter_mut()
                    .find(|entry| entry["id"] == json!(row.id))
                {
                    entry["settled_work"] = work;
                } else {
                    affected.push(
                        json!({"path":format!("id:{}",row.id),"id":row.id,"kind":"thread",
                        "action":asked.command.word(),"settled_work":work}),
                    );
                }
            }
            if !cleanup.is_empty() {
                affected[0]["environment_cleanup"] = json!(cleanup);
            }
        }
        if let Some(planned) = &draft {
            affected[0]["owner"] = json!(giver);
            affected[0]["name"] = json!(planned.request.name);
            affected[0]["history"] = serde_json::to_value(&planned.plan.history)?;
            affected[0]["workspace"] =
                json!("Uses the source environment and directory; no files are copied.");
            affected[0]["agent_access"] = json!("nothing");
            affected[0]["access"] = json!(
                "Source page overrides are copied; inherited rows follow destination defaults. The giver is first owner."
            );
        }
        let preview = json!({"done":false,"confirm":"00000000000000000000000000000000",
            "expires_in":self.inner.config.manage_plan_kept.as_secs(),"affects":affected,"refused":[]});
        if let Some(answer) = self.confirm_shared(call, asked, confirm, stamp, preview) {
            return Ok(Some(answer));
        }
        source::check(&mut tx).await?;
        let answer = match asked.command {
            Command::Fork => {
                let caller = Caller::session(giver).with_admin_mode(false);
                let Some(draft) = draft else {
                    return Err(EngineError::unreadable("the confirmed fork lost its draft"));
                };
                let job = match self
                    .start_agent_fork_job(
                        &mut tx,
                        &caller,
                        (call.thread, call.turn),
                        &selected,
                        draft,
                    )
                    .await
                {
                    Ok(job) => job,
                    Err(error) => return plan_error(error).map(Some),
                };
                json!({"done":true,"queued":true,"job_id":job.id,"owner":giver,"results":[{"path":path,"action":"fork"}]})
            }
            Command::Delete => {
                self.delete_thread_in(&mut tx, target, None, now)
                    .await
                    .map_err(EngineError::unreadable)?;
                self.audit_shared_thread(
                    &mut tx,
                    &asking,
                    orch_db::audit::action!(AuditAction::ThreadDeletedByAgent),
                    target,
                    giver,
                )
                .await?;
                json!({"done":true,"results":[{"path":path,"action":"delete"}]})
            }
            command => {
                self.lock_archive_in(&mut tx, target).await?;
                let archive = command == Command::Archive;
                let changed = if archive {
                    hierarchy::archive_by(&mut tx, target, None, now.get()).await?
                } else {
                    hierarchy::unarchive(&mut tx, target, now.get()).await?
                };
                if changed
                    || (archive
                        && orch_db::queries::lifecycle::archive_work_pending(&mut tx, target)
                            .await?)
                {
                    epoch::bump(&mut tx).await?;
                    if archive {
                        self.archive_as_in(&mut tx, target, None, now).await?;
                    }
                    hierarchy::stamp_subtree(&mut tx, target).await?;
                    self.audit_shared_thread(
                        &mut tx,
                        &asking,
                        if archive {
                            orch_db::audit::action!(AuditAction::ThreadArchivedByAgent)
                        } else {
                            orch_db::audit::action!(AuditAction::ThreadUnarchivedByAgent)
                        },
                        target,
                        giver,
                    )
                    .await?;
                }
                json!({"done":true,"results":[{"path":path,"action":command.word(),"changed":changed}]})
            }
        };
        self.finish_shared(tx, call, answer).await.map(Some)
    }

    /// None means a complete matching plan was consumed. Size refusals issue or spend no word.
    fn confirm_shared(
        &self,
        call: &CallContext,
        asked: &Asked,
        confirm: Option<&str>,
        stamp: Json,
        mut preview: Json,
    ) -> Option<Answer> {
        if !orch_agent_contract::tool_answers::fits("manage_shared", &call.arguments, &preview) {
            return Some(Refusal::new(Code::TooLarge, "The complete plan does not fit max_bytes. Nothing was done and no word was issued or spent; request a larger bound or narrow the selection.").into());
        }
        let now = self.inner.clock.now();
        if let Some(word) = confirm {
            return match lock(&self.inner.manage_plans).claim(call.thread, word, asked, now) {
                Claim::Claimed(waiting) if waiting.shared_stamp.as_ref() == Some(&stamp) => None,
                _ => Some(not_confirmed()),
            };
        }
        let word = new_word();
        preview["confirm"] = json!(word);
        lock(&self.inner.manage_plans).keep(
            call.thread,
            &word,
            Waiting {
                made: now,
                asked: asked.clone(),
                stamp: None,
                shared_stamp: Some(stamp),
            },
            now,
        );
        Some(Answer::Done(preview))
    }

    async fn finish_shared(
        &self,
        tx: Tx,
        call: &CallContext,
        answer: Json,
    ) -> Result<Answer, EngineError> {
        if !orch_agent_contract::tool_answers::fits("manage_shared", &call.arguments, &answer) {
            return Ok(Refusal::new(Code::TooLarge, "The complete outcome does not fit max_bytes. Nothing was changed; request a larger bound and make a new plan.").into());
        }
        tx.commit().await.map_err(DbError::from)?;
        self.kick();
        Ok(Answer::Done(answer))
    }

    async fn audit_shared_thread(
        &self,
        tx: &mut Tx,
        asking: &Asking,
        action: AuditAction,
        target: Id,
        giver: Id,
    ) -> Result<(), EngineError> {
        audit_by_agent(
            tx,
            asking,
            action,
            "thread",
            target,
            Some(target),
            "managed a shared thread",
            json!({"giver":giver,"source_thread":asking.agent.thread}),
        )
        .await
    }
}
