//! Referenced-by search lists what refers to a document, only through what the agent may read, with the lines around the mentions.

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
use orch_access_contract::pages::StoredKind;
use orch_api_contract::documents::{BlockContent, Inline, InlineRefKind};
use orch_api_contract::search::RelatedKind;
use orch_authz::reach::{AgentReach, DocumentPolicy};
use orch_db::queries::{links as rows, search::CandidateSet, threads::ThreadRow};
use orch_db::secrets::MasterKey;
use orch_documents::links::{self, Names, Target};
use orch_documents::references::{self, Typed, Writing};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::HashMap;
use std::sync::OnceLock;

/// How many sources a call lists when it does not say.
const RESULTS_DEFAULT: usize = 50;
/// How many mentions each listed source shows its lines for.
const LINES_PER_SOURCE: i64 = 3;

/// A cursor is transient search state: a process-local key hides skipped sources without retaining cursor rows.
static CURSOR_KEY: OnceLock<MasterKey> = OnceLock::new();

fn cursor_key() -> &'static MasterKey {
    CURSOR_KEY.get_or_init(|| MasterKey::new(rand::random()))
}

fn cursor_audience(thread: Id, document: Id, args: &Arguments) -> Result<Vec<u8>, EngineError> {
    serde_json::to_vec(&json!({
        "purpose":"referenced_by/v1","thread":thread,"document":document,
        "path":args.path.as_deref().unwrap_or("/"),"kinds":args.kinds,"names":args.names,
    }))
    .map_err(EngineError::unreadable)
}

fn cursor_after(text: &str, audience: &[u8]) -> Option<Id> {
    // A position contains one UUID and its authenticated envelope; use the existing query bound before decoding.
    if text.len() > crate::tools::definitions::QUERY_MAX_CHARS {
        return None;
    }
    let sealed = URL_SAFE_NO_PAD.decode(text).ok()?;
    let plain = cursor_key().open(&sealed, audience).ok()?;
    serde_json::from_slice(&plain).ok()
}

fn cursor_for(below: Id, audience: &[u8]) -> Result<String, EngineError> {
    let plain = serde_json::to_vec(&below).map_err(EngineError::unreadable)?;
    let sealed = cursor_key()
        .seal(&plain, audience)
        .map_err(EngineError::unreadable)?;
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

/// What an agent may call each chip of a line: only what its own reach lets it read; anything else is told as hidden.
struct AgentNames {
    chips: HashMap<(InlineRefKind, Id), String>,
}

impl Names for AgentNames {
    fn chip(&self, kind: InlineRefKind, id: Id, _path: Option<&str>) -> Option<String> {
        self.chips.get(&(kind, id)).cloned()
    }

    fn person(&self, _id: Id) -> Option<String> {
        None
    }

    fn hidden(&self, _kind: InlineRefKind, _label: Option<&str>) -> String {
        format!("({})", references::HIDDEN_WORDS)
    }
}

/// The kind of thing the edge store calls `document`, `chat` or `conversation`.
fn kind_of(word: &str) -> RelatedKind {
    match word {
        "chat" => RelatedKind::Chat,
        "conversation" => RelatedKind::Thread,
        _ => RelatedKind::Document,
    }
}

/// The typed references a part names, as they stand in its content.
fn typed_in_part(part: &rows::PartRow) -> Vec<Typed> {
    let mut found = links::typed_in_value(&part.body);
    let runs: Vec<Inline> = match part.part_kind.as_str() {
        "block" => match orch_documents::blocks::content_from_json(&part.body) {
            BlockContent::Inline(runs) => runs,
            BlockContent::Rows(rows) => rows
                .into_iter()
                .flat_map(|row| row.cells)
                .flat_map(|cell| cell.content)
                .collect(),
        },
        "comment" => serde_json::from_value(part.body.clone()).unwrap_or_default(),
        _ => Vec::new(),
    };
    for run in runs {
        if let Inline::Ref { kind, id, path, .. } = run {
            found.push(Typed {
                kind,
                id,
                path,
                part: None,
            });
        }
    }
    found
}

impl Engine {
    #[allow(
        clippy::too_many_lines,
        reason = "one walk over a few steps that are told apart where they matter: the document, the sources, their lines"
    )]
    pub(super) async fn referenced_by_call(
        &self,
        thread: &ThreadRow,
        args: &Arguments,
        source: &str,
    ) -> Result<Answer, EngineError> {
        if !args.query.is_empty()
            || args.regex.is_some()
            || args.context.is_some()
            || args.snippets.is_some()
            || args.related_to.is_some()
        {
            return Ok(Refusal::invalid("referenced_by lists whole entries. Omit query, regex, related_to, context and snippets; path, kinds, names and max_results may narrow its list. Continue with next_cursor and the same filters.").into());
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
        let source_selection = self
            .related_source_selection(&mut conn, thread.node_id, source)
            .await?;
        let fast = self
            .locate_shared_search(&mut conn, thread.node_id, source, &source_selection)
            .await?;
        let resolved = if fast.is_err() && !source.trim().starts_with("id:") {
            self.locate_shared(&mut conn, thread.node_id, source, false, false)
                .await?
        } else {
            fast
        };
        let located = match resolved {
            Ok(located) => located,
            Err(refusal) => return Ok(refusal.into()),
        };
        let FoundEntry::File(Where::Document(document)) = located.found.entry else {
            return Ok(Refusal::invalid(
                "referenced_by names a readable document, by its shared path or id:<id>.",
            )
            .into());
        };
        let Some(target) = orch_db::queries::documents::get(&mut conn, document)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Refusal::invalid(
                "The document is no longer available. List its directory again.",
            )
            .into());
        };
        let asking = located.asking;
        let audience = cursor_audience(thread.node_id, document, args)?;
        let mut below = match args.cursor.as_deref() {
            Some(text) => match cursor_after(text, &audience) {
                Some(position) => Some(position),
                None => return Ok(Refusal::invalid("This cursor belongs to another referenced_by list or the engine restarted. Omit cursor to start again.").into()),
            },
            None => None,
        };
        let place = Place::Node(thread.node_id);
        let count = |value: Option<i64>, fallback: usize| {
            value
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value > 0)
                .unwrap_or(fallback)
        };
        let checks = count(
            orch_settings::get(&mut conn, place, keys::DOCUMENTS_LINKS_ACCESS_CHECKS).await?,
            500,
        );
        let chars = count(
            orch_settings::get(&mut conn, place, keys::DOCUMENTS_LINKS_LINE_CHARS).await?,
            140,
        );
        let counted = count(
            orch_settings::get(&mut conn, place, keys::DOCUMENTS_LINKS_MENTIONS_COUNTED).await?,
            100,
        );
        let limit = args
            .max_results
            .and_then(|value| usize::try_from(value).ok())
            .map_or(RESULTS_DEFAULT, |value| value.max(1));
        // The scopes the agent reaches, for the conservative narrowing; each source is then looked up in its tree.
        let mut scopes = vec![thread.node_id];
        let mut documents = vec![document];
        let mut chats = Vec::new();
        if asking.documents_policy != DocumentPolicy::None || asking.policy == AgentReach::Topic {
            scopes.push(asking.agent.topic);
        }
        if asking.policy == AgentReach::Project {
            scopes.extend(asking.agent.project);
        }
        for grant in orch_authz::reach::grants(&mut conn, &asking.agent, asking.now).await? {
            match grant.page.kind {
                StoredKind::Node => scopes.push(grant.page.id),
                StoredKind::Document => documents.push(grant.page.id),
                StoredKind::Chat => chats.push(grant.page.id),
                _ => {}
            }
        }
        let readable = json!({"scopes":scopes,"documents":documents,"chats":chats,"groups":[]});
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
        let path = args.path.as_deref().unwrap_or("/");
        let mut delivered: Vec<(rows::LiveSource, String)> = Vec::new();
        let mut more = false;
        let mut examined = 0usize;
        'rounds: loop {
            let round = limit
                .saturating_mul(4)
                .max(100)
                .min(checks.saturating_sub(examined))
                .max(1);
            let ids = rows::raw_sources(
                &mut conn,
                target.id,
                below,
                i64::try_from(round).unwrap_or(i64::MAX),
            )
            .await?;
            let Some(&last) = ids.last() else {
                break;
            };
            let mut live = rows::live_sources(&mut conn, target.id, &ids, &readable).await?;
            live.sort_by_key(|source| std::cmp::Reverse(source.source_id));
            let mut selected = CandidateSet::default();
            for source in &live {
                match source.kind.as_str() {
                    "document" => selected.documents.push(source.source_id),
                    "chat" => selected.chats.push(source.source_id),
                    _ => selected.nodes.push(source.source_id),
                }
            }
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
            for source in live {
                let Some(shared) = paths.get(&source.source_id) else {
                    continue;
                };
                let inside = if directory {
                    shared.starts_with(scope)
                } else {
                    shared == scope
                };
                let named_ok = name
                    .as_ref()
                    .is_none_or(|name| name.matches(shared.rsplit('/').next().unwrap_or(shared)));
                if !inside || !named_ok || !wanted(kind_of(&source.kind)) {
                    continue;
                }
                if delivered.len() == limit {
                    more = true;
                    // The look-ahead source was not delivered. Continue after the last one actually shown.
                    below = delivered.last().map(|(source, _)| source.source_id);
                    break 'rounds;
                }
                delivered.push((source, shared.clone()));
            }
            examined += ids.len();
            below = Some(last);
            if ids.len() < round {
                break;
            }
            if examined >= checks {
                more = true;
                break;
            }
        }
        let chosen: Vec<rows::LiveSource> =
            delivered.iter().map(|(source, _)| source.clone()).collect();
        let (parts, counts) = if chosen.is_empty() {
            (Vec::new(), HashMap::new())
        } else {
            // One more than is counted says whether the count reads "or more".
            let limit = i64::try_from(counted).unwrap_or(i64::MAX).saturating_add(1);
            (
                rows::parts(&mut conn, target.id, &chosen, LINES_PER_SOURCE, None).await?,
                rows::mention_counts(&mut conn, target.id, &chosen, limit).await?,
            )
        };
        let typed: Vec<Typed> = parts.iter().flat_map(typed_in_part).collect();
        let written = self.writings_for(&mut conn, thread.node_id, &typed).await?;
        let names = AgentNames {
            chips: written
                .into_iter()
                .filter_map(|(typed, writing)| match writing {
                    Writing::Link { name, .. } => Some(((typed.kind, typed.id), name)),
                    Writing::Hidden(_) => None,
                })
                .collect(),
        };
        let reference = Target {
            kind: InlineRefKind::Document,
            id: target.id,
        };
        let mut hits: Vec<Json> = delivered
            .iter()
            .map(|(source, shared)| {
                let mine: Vec<&rows::PartRow> = parts
                    .iter()
                    .filter(|part| part.source_id == source.source_id)
                    .collect();
                let lines: Vec<Json> = mine
                    .iter()
                    .map(|part| {
                        let line = match part.part_kind.as_str() {
                            "block" => links::line_in_content(
                                &orch_documents::blocks::content_from_json(&part.body),
                                reference,
                                &target.title,
                                &names,
                                chars,
                            ),
                            "comment" => serde_json::from_value::<Vec<Inline>>(part.body.clone())
                                .ok()
                                .and_then(|runs| {
                                    links::line_in_runs(
                                        &runs,
                                        reference,
                                        &target.title,
                                        &names,
                                        chars,
                                    )
                                }),
                            _ => None,
                        }
                        .or_else(|| {
                            links::line_in_value(
                                &part.body,
                                reference,
                                &target.title,
                                &names,
                                chars,
                            )
                        })
                        .or_else(|| links::opening_of_value(&part.body, chars));
                        let text = line.map_or_else(String::new, |line| {
                            if line.reference.is_empty() {
                                format!("{}{}", line.before, line.after)
                            } else {
                                format!("{}[{}]{}", line.before, line.reference, line.after)
                            }
                        });
                        let where_ = match part.part_kind.as_str() {
                            "comment" => "comment",
                            "message" => "message",
                            "item" => "turn",
                            _ => "text",
                        };
                        json!({"in":where_,"turn":part.turn,"text":text})
                    })
                    .collect();
                let found = counts.get(&source.source_id).copied().unwrap_or(0);
                let cap = i64::try_from(counted).unwrap_or(i64::MAX);
                json!({
                    "id":source.source_id,"path":shared,"kind":source.kind,
                    "mentions":found.min(cap),"mentions_more":found > cap,
                    "lines":lines,"position":null,
                })
            })
            .collect();
        // A model-visible answer may be cut between hits; a cursor on each hit preserves every unshown source.
        for (hit, (source, _)) in hits.iter_mut().zip(&delivered) {
            hit["cursor"] = json!(cursor_for(source.source_id, &audience)?);
        }
        let next_cursor = if more {
            below
                .map(|position| cursor_for(position, &audience))
                .transpose()?
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
            "An agent looked for what refers to a shared document.",
            json!({"referenced_by":located.found.path,"hits":hits.len()}),
        )
        .await?;
        cancellation.disarm();
        conn.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Answer::Done(json!({
            "referenced_by":located.found.path,"hits":hits,
            "truncated":more,"partial":more,"next_cursor":next_cursor,
            "searched":{"entries":hits.len(),"files":0,"bytes":0},"skipped":[],
        })))
    }
}
