//! Native turn-boundary checkpoints, retaining the harness's paginated lineage.

use super::*;
use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
use std::collections::HashSet;
use std::path::PathBuf;

mod checkpoint;
#[cfg(test)]
mod tests;

fn unsupported(words: &str) -> AdapterError {
    AdapterError::Unsupported(words.into())
}

fn goal_reply(answer: &Json) -> Result<(), AdapterError> {
    match answer.get("goal") {
        Some(Json::Null) => Ok(()),
        Some(_) => Err(unsupported(
            "This native source has a goal outside its retained input history; it cannot provide this checkpoint.",
        )),
        None => Err(invalid(
            "The native goal response does not declare its state.",
        )),
    }
}

fn goal_state(answer: Result<Json, RpcError>, home: HomeFacts) -> Result<(), AdapterError> {
    match answer {
        Ok(answer) => goal_reply(&answer),
        Err(RpcError::Remote {
            code: -32600,
            message,
            ..
        }) if message == "goals feature is disabled" => {
            if home.goals_disabled_by_orch {
                // A disabled goal mechanism cannot run a persisted goal in this configured home.
                Ok(())
            } else {
                Err(unsupported(
                    "The harness has disabled its goal-state API; this source's external goal state cannot be verified.",
                ))
            }
        }
        Err(error) => Err(map_rpc(error)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    item: String,
    client: Option<String>,
    turn: String,
}
impl Selection {
    fn identity(&self) -> &str {
        self.client.as_deref().unwrap_or(&self.item)
    }
    fn matches(&self, item: &Json, client: Option<&str>) -> bool {
        item["id"] == self.item
            || self
                .client
                .as_deref()
                .is_some_and(|expected| client == Some(expected))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Checkpoint {
    pub prefix: NativeInputPrefix,
    pub turn: String,
    pub paginated: bool,
}

/// The legacy projection may use item-N, but its retained client identity is
/// separate. Canonical paginated user items retain their actual native IDs.
fn native_user_event(row: &Json) -> Option<(&Json, Option<&str>)> {
    if row["type"] != "event_msg" {
        return None;
    }
    let event = &row["payload"];
    if event["type"] == "user_message" {
        return Some((event, event["client_id"].as_str()));
    }
    if matches!(
        event["type"].as_str(),
        Some("item_started" | "item_completed")
    ) && event["item"]["type"] == "UserMessage"
    {
        return Some((&event["item"], event["item"]["client_id"].as_str()));
    }
    None
}

#[derive(Default)]
struct Membership {
    first: HashSet<String>,
    ids: HashSet<String>,
    selected: Option<Selection>,
}
impl Membership {
    fn page(&mut self, rows: &[Json], before: &str) -> Result<(), AdapterError> {
        for row in rows {
            let item = &row["item"];
            if item["type"] != "userMessage" {
                continue;
            }
            let id = item["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| invalid("A native user item has no identity."))?;
            let turn = row["turnId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| invalid("A native user item has no owning turn."))?;
            if !self.ids.insert(id.into()) {
                return Err(invalid("The native API repeats a user-input identity."));
            }
            let first = self.first.insert(turn.into());
            if item["id"] == before || item["clientId"] == before {
                if self.selected.is_some() {
                    return Err(invalid(
                        "The native API contains an ambiguous user-input identity.",
                    ));
                }
                if !first {
                    return Err(unsupported(
                        "Codex forks at native turn boundaries; this input was added inside its turn.",
                    ));
                }
                self.selected = Some(Selection {
                    item: id.into(),
                    client: item["clientId"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned),
                    turn: turn.into(),
                });
            }
        }
        Ok(())
    }
}

impl CodexAdapter {
    async fn native_thread(&self, id: &str) -> Result<Json, AdapterError> {
        let (shared, _) = self.live()?;
        let answer = shared
            .rpc()
            .call("thread/read", json!({"threadId":id,"includeTurns":false}))
            .await
            .map_err(map_rpc)?;
        let thread = answer
            .get("thread")
            .ok_or_else(|| invalid("The native source has no thread metadata."))?;
        if thread["id"] != id {
            return Err(conflict(
                "The harness returned another native source session.",
            ));
        }
        if !matches!(thread["historyMode"].as_str(), Some("legacy" | "paginated")) {
            return Err(unsupported(
                "This native source declares an unknown history mode.",
            ));
        }
        Ok(thread.clone())
    }

    async fn goal_free(&self, id: &str) -> Result<(), AdapterError> {
        let (shared, running) = self.live()?;
        let answer = shared
            .rpc()
            .call("thread/goal/get", json!({"threadId":id}))
            .await;
        goal_state(answer, running.home_facts)
    }

    async fn membership(&self, id: &str, before: &str) -> Result<Selection, AdapterError> {
        if before.is_empty() {
            return Err(invalid(
                "An exact checkpoint names a retained native user input.",
            ));
        }
        let (shared, _) = self.live()?;
        let mut membership = Membership::default();
        let mut cursor: Option<String> = None;
        let mut cursors = HashSet::new();
        loop {
            let page = shared
                .rpc()
                .call(
                    "thread/items/list",
                    json!({"threadId":id,"sortDirection":"asc",
                "limit":TURNS_PAGE,"cursor":cursor}),
                )
                .await
                .map_err(map_rpc)?;
            membership.page(
                page["data"]
                    .as_array()
                    .ok_or_else(|| invalid("The native item page is invalid."))?,
                before,
            )?;
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            let Some(next) = cursor.as_ref() else {
                break;
            };
            if !cursors.insert(next.clone()) {
                return Err(invalid("The native item page repeats its cursor."));
            }
        }
        let selected = membership.selected.ok_or_else(|| {
            invalid("That native user input is not retained in the current source history.")
        })?;
        cursor = None;
        cursors.clear();
        let mut found = false;
        loop {
            let page = shared
                .rpc()
                .call(
                    "thread/turns/list",
                    json!({"threadId":id,"sortDirection":"asc",
                "itemsView":"notLoaded","limit":TURNS_PAGE,"cursor":cursor}),
                )
                .await
                .map_err(map_rpc)?;
            for turn in page["data"]
                .as_array()
                .ok_or_else(|| invalid("The native turn page is invalid."))?
            {
                if turn["id"] == selected.turn {
                    if found {
                        return Err(invalid("The native API repeats the selected turn."));
                    }
                    if !matches!(
                        turn["status"].as_str(),
                        Some("completed" | "failed" | "interrupted")
                    ) {
                        return Err(conflict(
                            "The selected native turn has no confirmed terminal state.",
                        ));
                    }
                    found = true;
                }
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            let Some(next) = cursor.as_ref() else {
                break;
            };
            if !cursors.insert(next.clone()) {
                return Err(invalid("The native turn page repeats its cursor."));
            }
        }
        if !found {
            return Err(invalid(
                "The selected native turn is not visible in this source.",
            ));
        }
        Ok(selected)
    }

    pub(super) async fn resolve_checkpoint(
        &self,
        source: NativePrefixSource,
        before: &str,
    ) -> Result<Checkpoint, AdapterError> {
        let (shared, running) = self.live()?;
        if running.started.harness_version != "0.165.0" {
            return Err(unsupported(
                "Exact input checkpoints require Codex 0.165.0.",
            ));
        }
        if let Ok((native, segment, busy)) = shared.with_session(source.thread_id, |record| {
            (
                record.mapper.native_thread().to_owned(),
                record.mapper.segment_id(),
                record.mapper.turn_in_progress(),
            )
        }) {
            if native != source.native_session_ref || segment != source.segment_id {
                return Err(conflict(
                    "The source lane now names another native session or segment.",
                ));
            }
            if busy {
                return Err(conflict(
                    "The source session has a running turn. Stop it and wait for its native end.",
                ));
            }
        } else if !self.prefix_sources.contains(&source) {
            return Err(AdapterError::Other {code:"session.source_unobserved".into(),retryable:true,
                message:"This source lane has no current idle or idle-close observation in this harness process.".into()});
        }
        let thread = self.native_thread(&source.native_session_ref).await?;
        let paginated = thread["historyMode"] == "paginated";
        if !matches!(
            thread["status"]["type"].as_str(),
            Some("idle" | "notLoaded")
        ) {
            return Err(conflict(
                "The native source has no confirmed quiescent state.",
            ));
        }
        self.goal_free(&source.native_session_ref).await?;
        let selected = self.membership(&source.native_session_ref, before).await?;
        let home = running
            .home
            .clone()
            .ok_or_else(|| unsupported("This harness has no retained native history home."))?;
        let mut metadata = thread;
        let mut id = source.native_session_ref.clone();
        let mut end = None;
        let mut seen = HashSet::new();
        let mut segments = Vec::new();
        loop {
            if !seen.insert(id.clone()) {
                return Err(invalid("The native inherited history contains a cycle."));
            }
            let path = metadata["path"]
                .as_str()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| {
                    invalid("A native ancestor has no retained absolute rollout path.")
                })?;
            let root = home.clone();
            let expected = id.clone();
            let limit = end;
            let segment = tokio::task::spawn_blocking(move || {
                checkpoint::segment(&root, &path, &expected, limit)
            })
            .await
            .map_err(|_| invalid("Reading native ancestry did not finish."))??;
            if metadata["historyMode"].as_str()
                != Some(if segment.paginated {
                    "paginated"
                } else {
                    "legacy"
                })
            {
                return Err(conflict(
                    "The native API and retained ancestry declare different history modes.",
                ));
            }
            let base = segment.base.clone();
            segments.push(segment);
            let Some(base) = base else {
                break;
            };
            id = base.thread_id.clone();
            end = Some(base);
            // HistoryPosition names a rollout ID. A reverted physical rollout
            // may differ from its thread ID; exact ownership then refuses.
            metadata = self.native_thread(&id).await?;
            if metadata["historyMode"] != "paginated" {
                return Err(invalid(
                    "A paginated native ancestor has another persistence mode.",
                ));
            }
        }
        segments.reverse();
        let requested = source.clone();
        let selection = selected.clone();
        let (fingerprint, empty) = tokio::task::spawn_blocking(move || {
            checkpoint::fingerprint(&segments, &selection, &requested)
        })
        .await
        .map_err(|_| invalid("Fingerprinting the native checkpoint did not finish."))??;
        Ok(Checkpoint {
            prefix: NativeInputPrefix {
                harness_kind: KIND.into(),
                source,
                before_input_ref: selected.identity().into(),
                fingerprint,
                empty,
            },
            turn: selected.turn,
            paginated,
        })
    }

    pub(super) async fn inspect_prefix(
        &mut self,
        source: NativePrefixSource,
        before: &str,
    ) -> Result<NativeInputPrefix, AdapterError> {
        Ok(self.resolve_checkpoint(source, before).await?.prefix)
    }

    pub(super) async fn open_prefix(
        &mut self,
        mut spec: SessionSpec,
        prefix: NativeInputPrefix,
    ) -> Result<NativePrefixOpened, AdapterError> {
        if prefix.harness_kind != KIND
            || spec.settings.mode != SessionMode::Fork
            || spec.settings.thread_id != prefix.source.thread_id
            || spec.settings.segment_id == prefix.source.segment_id
            || spec.settings.native_session_ref.as_deref()
                != Some(prefix.source.native_session_ref.as_str())
            || spec.settings.upto_turn_ordinal.is_some()
        {
            return Err(invalid(
                "An input-prefix open names its exact source and no ordinal cut.",
            ));
        }
        let (shared, _) = self.live()?;
        if shared.with_session(spec.settings.thread_id, |_| ()).is_ok() {
            return Err(conflict(
                "The target session lane is occupied. Close it before opening the checkpoint.",
            ));
        }
        let current = self
            .resolve_checkpoint(prefix.source.clone(), &prefix.before_input_ref)
            .await?;
        if current.prefix != prefix {
            return Err(conflict(
                "The retained native checkpoint changed after inspection.",
            ));
        }
        validate_session_open(&spec)?;
        shared
            .unload_before_resume(&prefix.source.native_session_ref)
            .await?;
        if prefix.empty {
            spec.settings.mode = SessionMode::New;
            spec.settings.native_session_ref = None;
        }
        let opened = self.open(spec, Some(&current), None).await?;
        self.prefix_sources.retain(|source| {
            source.thread_id != prefix.source.thread_id
                && source.native_session_ref != opened.native_session_ref
        });
        Ok(NativePrefixOpened {
            session: opened,
            prefix,
        })
    }
}
