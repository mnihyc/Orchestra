//! Related search uses the shared weighted ranking and the agent's exact current tree for every delivered path.

use super::Arguments;
use crate::{
    engine::Engine,
    error::EngineError,
    tools::{
        Answer,
        definitions::Refusal,
        tree::{FoundEntry, Pattern, Where},
    },
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_api_contract::search::RelatedKind;
use orch_authz::reach::{AgentReach, DocumentPolicy};
use orch_db::queries::{related as rows, search::CandidateSet, threads::ThreadRow};
use orch_db::{Conn, DbError};
use orch_types::{Hash, Id, Json};
use serde_json::json;
use std::collections::HashMap;

#[derive(serde::Serialize, serde::Deserialize)]
struct RelatedContinuation {
    selection: Hash,
    window: Hash,
    offset: usize,
}

impl Engine {
    pub(super) async fn related_shared_call(
        &self,
        thread: &ThreadRow,
        args: &Arguments,
        source: &str,
    ) -> Result<Answer, EngineError> {
        if !args.query.is_empty()
            || args.regex.is_some()
            || args.context.is_some()
            || args.snippets.is_some()
        {
            return Ok(Refusal::invalid("related_to returns whole entries. Omit query, regex, context and snippets; path, kinds, names and max_results may narrow its list.").into());
        }
        let name = match args.names.as_deref().map(Pattern::new) {
            Some(None) => {
                return Ok(
                    Refusal::invalid("\"names\" takes *, ? and [...] as a shell pattern.").into(),
                );
            }
            Some(Some(pattern)) => Some(pattern),
            None => None,
        };
        let mut conn = self.db().begin_snapshot().await?;
        let mut cancellation =
            orch_db::queries::search::cancel_on_drop(self.db(), &mut conn).await?;
        let mut source_selection = self
            .related_source_selection(&mut conn, thread.node_id, source)
            .await?;
        source_selection.headers_only = true;
        let fast = self
            .locate_shared_search(&mut conn, thread.node_id, source, &source_selection)
            .await?;
        let resolved = fast;
        let located = match resolved {
            Ok(located) => located,
            Err(refusal) => return Ok(refusal.into()),
        };
        let FoundEntry::File(Where::Document(document)) = located.found.entry else {
            return Ok(Refusal::invalid(
                "related_to names a readable document, by its shared path or id:<id>.",
            )
            .into());
        };
        let Some(source) = rows::items(&mut conn, &[document])
            .await?
            .into_iter()
            .find(|item| item.kind == "document")
        else {
            return Ok(Refusal::invalid(
                "The related document is no longer available. List its directory again.",
            )
            .into());
        };
        let asking = located.asking;
        let mut scopes = vec![thread.node_id];
        let mut documents = vec![document];
        let mut chats: Vec<Id> = Vec::new();
        if asking.documents_policy != DocumentPolicy::None || asking.policy == AgentReach::Topic {
            scopes.push(asking.agent.topic);
        }
        if asking.policy == AgentReach::Project {
            scopes.extend(asking.agent.project);
        }
        let budget = orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::Node(source.node_id),
            orch_settings_contract::registry::keys::RELATED_CANDIDATES,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("related candidates are absent"))?;
        let (entrances, complete) =
            rows::agent_entrances(&mut conn, &asking.agent.path, asking.now.get(), budget).await?;
        for (kind, id) in entrances {
            match kind.as_str() {
                "node" => scopes.push(id),
                "document" => documents.push(id),
                "chat" => chats.push(id),
                _ => {}
            }
        }
        let readable = json!({"scopes":scopes,"documents":documents,"chats":chats,"groups":[],
            "agent_path":asking.agent.path,"at":asking.now.get(),"agent_grants_complete":complete});
        let mut candidates = self
            .related_candidates(&mut conn, &source, &readable)
            .await?;
        let limit = usize::try_from(
            args.max_results
                .unwrap_or(i64::try_from(candidates.count).unwrap_or(i64::MAX))
                .clamp(1, super::RESULTS_MAX),
        )
        .unwrap_or(1);
        let mut selected = CandidateSet {
            headers_only: true,
            ..Default::default()
        };
        for candidate in &candidates.items {
            match candidate.item.kind.as_str() {
                "document" => {
                    selected.documents.push(candidate.item.source_id);
                }
                "chat" => {
                    selected.chats.push(candidate.item.source_id);
                }
                _ => {
                    selected.nodes.push(candidate.item.source_id);
                }
            }
        }
        let path = args.path.as_deref().unwrap_or("/");
        let scoped = match self
            .locate_shared_search(&mut conn, thread.node_id, path, &selected)
            .await?
        {
            Ok(scoped) => scoped,
            Err(refusal) => return Ok(refusal.into()),
        };
        let mut paths: HashMap<Id, String> = scoped
            .tree
            .documents()
            .into_iter()
            .map(|(doc, path)| (doc.row.id, path))
            .collect();
        paths.extend(
            scoped
                .tree
                .conversations()
                .into_iter()
                .map(|(id, _, entry)| (id, entry.path)),
        );
        paths.extend(
            scoped
                .tree
                .chats()
                .into_iter()
                .map(|(id, path, _)| (id, path)),
        );
        let directory = matches!(scoped.found.entry, FoundEntry::Directory(_));
        let scope = &scoped.found.path;
        let wanted = |kind: RelatedKind| {
            args.kinds.as_ref().is_none_or(|kinds| {
                kinds.iter().any(|word| {
                    word == match kind {
                        RelatedKind::Document => "documents",
                        RelatedKind::Chat => "chats",
                        RelatedKind::Thread => "conversations",
                    }
                })
            })
        };
        candidates.items.retain(|candidate| {
            paths.get(&candidate.item.source_id).is_some_and(|path| {
                (if directory {
                    path.starts_with(scope)
                } else {
                    path == scope
                }) && wanted(candidate.result.kind)
                    && name
                        .as_ref()
                        .is_none_or(|name| name.matches(path.rsplit('/').next().unwrap_or(path)))
            })
        });
        let unnamed: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM chats WHERE id=ANY($1) AND node_id IS NULL AND name IS NULL",
        )
        .bind(&selected.chats)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        for id in unnamed {
            if paths.contains_key(&id) {
                paths.insert(id, format!("id:{id}"));
            }
        }
        let selection = Hash::of(
            &serde_json::to_vec(&json!({"thread":thread.node_id,"source":source.source_id,
            "seq":source.marker,"path":args.path,"kinds":args.kinds,"names":args.names}))
            .map_err(EngineError::unreadable)?,
        );
        let mut digest = Hash::hasher();
        digest.update(
            &serde_json::to_vec(&(
                candidates.count,
                &candidates.meaning,
                &candidates.meaning_reason,
            ))
            .map_err(EngineError::unreadable)?,
        );
        for item in candidates.items.iter() {
            digest.update(
                &serde_json::to_vec(&(&item.result, paths.get(&item.item.source_id)))
                    .map_err(EngineError::unreadable)?,
            );
        }
        let window = Hash::finish(&digest);
        let offset = if let Some(token) = args.cursor.as_deref() {
            let continued = URL_SAFE_NO_PAD
                .decode(token)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<RelatedContinuation>(&bytes).ok());
            let Some(continued) =
                continued.filter(|cursor| cursor.selection == selection && cursor.window == window)
            else {
                return Ok(Refusal::invalid(
                    "The related list or its access changed. Start again without cursor.",
                )
                .into());
            };
            continued.offset
        } else {
            0
        };
        let searched = i64::try_from(candidates.items.len()).unwrap_or(i64::MAX);
        let mut hits: Vec<Json> = candidates
            .items
            .into_iter()
            .skip(offset)
            .take(limit)
            .filter_map(|candidate| {
                paths.get(&candidate.item.source_id).map(|path| {
                    json!({
                        "id":candidate.item.source_id,"path":path,"kind":candidate.item.kind,
                        "related_by":candidate.result.related_by,"place":candidate.result.place,
                        "references":candidate.result.references,"position":null,
                    })
                })
            })
            .collect();
        for (shown, hit) in hits.iter_mut().enumerate() {
            hit["cursor"] = Json::String(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&RelatedContinuation {
                        selection,
                        window,
                        offset: offset.saturating_add(shown).saturating_add(1),
                    })
                    .map_err(EngineError::unreadable)?,
                ),
            );
        }
        let more = offset.saturating_add(hits.len()) < usize::try_from(searched).unwrap_or(0);
        let next_cursor = if more {
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&RelatedContinuation {
                        selection,
                        window,
                        offset: offset.saturating_add(hits.len()),
                    })
                    .map_err(EngineError::unreadable)?,
                ),
            )
        } else {
            None
        };
        crate::tools::shared::audit_by_agent(
            &mut conn,
            &asking,
            orch_db::audit::action!(orch_access_contract::audit::AuditAction::ShareSearchedByAgent),
            "thread",
            asking.agent.thread,
            Some(asking.agent.thread),
            "An agent looked for related shared items.",
            json!({"related_to":located.found.path,"hits":hits.len()}),
        )
        .await?;
        cancellation.disarm();
        conn.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Answer::Done(
            json!({"related_to":located.found.path,"hits":hits,
            "meaning":{"state":candidates.meaning,"reason":candidates.meaning_reason},
            "truncated":more,"partial":false,"next_cursor":next_cursor,
            "searched":{"entries":searched,"files":0,"bytes":0},"skipped":[]}),
        ))
    }

    pub(super) async fn related_source_selection(
        &self,
        conn: &mut Conn,
        thread: Id,
        source: &str,
    ) -> Result<CandidateSet, EngineError> {
        use crate::tools::tree::{self, Named, Scope};
        let Some(named) = tree::parse_path(source) else {
            return Ok(CandidateSet::default());
        };
        if let Named::Id(id) = named {
            return Ok(CandidateSet {
                documents: vec![id],
                nodes: vec![id],
                chats: vec![id],
                ..Default::default()
            });
        }
        let Some(asking) = self.asking(conn, thread).await? else {
            return Ok(CandidateSet::default());
        };
        let absolute = tree::absolute_headers(conn, &asking.agent, &named).await?;
        let walked = tree::walk_headers(conn, &absolute).await?;
        let (Scope::Under(node), Named::Path { parts, .. }) = (walked.scope, absolute) else {
            return Ok(CandidateSet::default());
        };
        let remaining = &parts[walked.consumed..];
        let Some((last, folders)) = remaining.get(1..).and_then(|parts| parts.split_last()) else {
            return Ok(CandidateSet::default());
        };
        let mut parent: Option<Id> = None;
        for folder in folders {
            // Select by stored namespace identity, then let the ordinary shared resolver check every path and
            // ancestor. No whole document inventory is hydrated just to resolve one known filename.
            let structural = folder
                .strip_prefix("_private-folder~")
                .and_then(|id| id.parse::<Id>().ok());
            let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM document_containers WHERE node_id=$1
                AND parent_container_id IS NOT DISTINCT FROM $2 AND deleted_at IS NULL
                AND ((orch_related_name_key(translate(name,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))=orch_related_name_key(translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
                AND translate(name,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')) OR id=$4) LIMIT 2")
                .bind(node).bind(parent).bind(folder).bind(structural).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            if ids.len() != 1 {
                return Ok(CandidateSet::default());
            }
            parent = ids.first().copied();
        }
        let documents = sqlx::query_scalar(sqlx::AssertSqlSafe(rows::source_documents_sql(
            parent.is_some(),
        )))
        .persistent(false)
        .bind(node)
        .bind(parent)
        .bind(last)
        .bind(last.strip_suffix(".md").unwrap_or(last))
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?;
        Ok(CandidateSet {
            documents,
            ..Default::default()
        })
    }
}
