//! What refers to a document, with the line around each mention, and what the document refers to.

use super::placed::{CastNames, Placed, kind_of, placed, store_kind};
use super::{Limits, Position, live_document, open, seal};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::documents::build::{Cast, Wanted};
use crate::routes::nodes::invalid;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_api_contract::common::Page;
use orch_api_contract::documents::{BlockContent, Inline, InlineRefKind};
use orch_api_contract::links::{
    DocumentReferencesListQuery, LinkAnchor, LinkDirection, LinkLine, LinkOccurrence, LinkedKind,
    LinkedThing,
};
use orch_api_contract::nodes::NodeKind;
use orch_authz::{Policy, Principal};
use orch_db::queries::documents::DocumentRow;
use orch_db::queries::links as rows;
use orch_db::{Conn, DbError};
use orch_documents::links::{self, Target};
use orch_engine::links::{Person, SourcePageAsk};
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};

/// What one source lists: its thing, with a cursor for the mentions that follow those listed.
struct Listed {
    thing: LinkedThing,
}

/// The word `of` names a kind of source by.
fn of_word(kind: LinkedKind) -> &'static str {
    match kind {
        LinkedKind::Chat => "chat",
        LinkedKind::Thread => "thread",
        _ => "document",
    }
}

pub(super) async fn list(
    ctx: Ctx,
    query: DocumentReferencesListQuery,
    (): (),
) -> ApiResult<Page<LinkedThing>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let mut cancellation = orch_db::queries::search::cancel_on_drop(&ctx.app.db, &mut tx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let document = live_document(&mut tx, ctx.id("id")?).await?;
    let limits = Limits::read(&mut tx, document.node_id).await?;
    let page = match query.direction {
        LinkDirection::In => incoming(&ctx, &mut tx, &principal, &document, &query, limits).await?,
        LinkDirection::Out => {
            outgoing(&ctx, &mut tx, &principal, &document, &query, limits).await?
        }
    };
    cancellation.disarm();
    tx.commit().await.map_err(DbError::from)?;
    Ok(page)
}

fn wrong_cursor() -> ApiError {
    invalid("cursor", "is not a cursor of this list")
}

/// The conservative scopes a person's sources are narrowed by before each is checked exactly.
async fn scopes_of(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
) -> ApiResult<orch_types::Json> {
    Ok(
        orch_authz::everything::project_search(conn, principal, None, &[document], &[], &[])
            .await
            .map_err(ApiError::internal)?
            .search_candidates(principal),
    )
}

async fn incoming(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    document: &DocumentRow,
    query: &DocumentReferencesListQuery,
    limits: Limits,
) -> ApiResult<Page<LinkedThing>> {
    if let Some(of) = query.of.as_deref() {
        return one_source(ctx, tx, principal, document, query, limits, of).await;
    }
    let before = match query
        .cursor
        .as_deref()
        .map(|text| open(principal, document.id, LinkDirection::In, None, text))
        .transpose()?
    {
        None => None,
        Some(Position::Sources { below }) => Some(below),
        Some(_) => return Err(wrong_cursor()),
    };
    let scopes = scopes_of(tx, principal, document.id).await?;
    let page = orch_engine::links::sources_page(
        tx,
        &Person(principal),
        SourcePageAsk {
            target: document.id,
            scopes: &scopes,
            before,
            limit: limits.page(query.limit),
            checks: limits.access_checks,
        },
    )
    .await
    .map_err(ApiError::internal)?;
    let listed = sources(ctx, tx, principal, document, &page.sources, limits, None).await?;
    let next_cursor = page
        .next
        .map(|below| {
            seal(
                principal,
                document.id,
                LinkDirection::In,
                None,
                &Position::Sources { below },
            )
        })
        .transpose()?;
    Ok(Page {
        items: listed.into_iter().map(|each| each.thing).collect(),
        next_cursor,
    })
}

/// The next mentions of one source, which the reader must be able to read like any other.
async fn one_source(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    document: &DocumentRow,
    query: &DocumentReferencesListQuery,
    limits: Limits,
    of: &str,
) -> ApiResult<Page<LinkedThing>> {
    let bad = || invalid("of", "is not a source, as kind:id");
    let (kind, id) = of.split_once(':').ok_or_else(bad)?;
    let kind = match kind {
        "document" => orch_api_contract::search::RelatedKind::Document,
        "chat" => orch_api_contract::search::RelatedKind::Chat,
        "thread" => orch_api_contract::search::RelatedKind::Thread,
        _ => return Err(bad()),
    };
    let id: Id = id.parse().map_err(|_| bad())?;
    let source = rows::LiveSource {
        kind: store_kind(kind).to_owned(),
        source_id: id,
    };
    let after = match query
        .cursor
        .as_deref()
        .map(|text| open(principal, document.id, LinkDirection::In, Some(of), text))
        .transpose()?
    {
        None => None,
        Some(Position::Mentions { part }) => Some(part),
        Some(_) => return Err(wrong_cursor()),
    };
    // A source the reader may not read has no mentions to give: the answer is the same as for one with none left.
    let readable =
        orch_engine::links::readable_by_person(tx, principal, std::slice::from_ref(&source))
            .await
            .map_err(ApiError::internal)?;
    if !readable.contains(&id) {
        return Ok(Page::last(Vec::new()));
    }
    let listed = sources(ctx, tx, principal, document, &[source], limits, after).await?;
    let next_cursor = listed
        .first()
        .and_then(|each| each.thing.next_mentions.clone());
    Ok(Page {
        items: listed.into_iter().map(|each| each.thing).collect(),
        next_cursor,
    })
}

fn part_of(anchor: &LinkAnchor) -> Option<Id> {
    match anchor {
        LinkAnchor::Block { block_id } => Some(*block_id),
        LinkAnchor::Comment { comment_id, .. } => Some(*comment_id),
        LinkAnchor::Message { message_id } => Some(*message_id),
        LinkAnchor::Turn { item_id, .. } => Some(*item_id),
    }
}

/// The names that the lines of some parts need, for one reader.
async fn names_for(
    tx: &mut Conn,
    principal: &Principal,
    parts: &[rows::PartRow],
) -> ApiResult<Cast> {
    let mut wanted = Wanted::default();
    for part in parts {
        if let Some(user) = part.by_user {
            wanted.user(user);
        }
        if let Some(thread) = part.by_thread {
            wanted.node(thread);
        }
        match part.part_kind.as_str() {
            "block" => wanted.content(&orch_documents::blocks::content_from_json(&part.body)),
            "comment" => {
                if let Ok(runs) = serde_json::from_value::<Vec<Inline>>(part.body.clone()) {
                    wanted.runs(&runs);
                }
            }
            _ => {}
        }
        for typed in links::typed_in_value(&part.body) {
            match typed.kind {
                InlineRefKind::Node => wanted.node(typed.id),
                InlineRefKind::Document => wanted.document(typed.id),
                InlineRefKind::Chat => wanted.chat(typed.id),
                InlineRefKind::File => wanted.file(typed.id),
                InlineRefKind::EnvironmentFile => wanted.environment(typed.id),
            }
        }
    }
    Cast::load(tx, principal, wanted, Policy::default()).await
}

/// The line around the mention in one part, as this reader reads it.
fn line_of(
    part: &rows::PartRow,
    document: &DocumentRow,
    names: &CastNames<'_>,
    chars: usize,
) -> LinkLine {
    let target = Target {
        kind: InlineRefKind::Document,
        id: document.id,
    };
    let found = match part.part_kind.as_str() {
        "block" => links::line_in_content(
            &orch_documents::blocks::content_from_json(&part.body),
            target,
            &document.title,
            names,
            chars,
        ),
        "comment" => serde_json::from_value::<Vec<Inline>>(part.body.clone())
            .ok()
            .and_then(|runs| links::line_in_runs(&runs, target, &document.title, names, chars)),
        _ => None,
    }
    .or_else(|| links::line_in_value(&part.body, target, &document.title, names, chars))
    .or_else(|| links::opening_of_value(&part.body, chars));
    found.map_or_else(
        || LinkLine {
            before: String::new(),
            reference: String::new(),
            after: String::new(),
        },
        |line| LinkLine {
            before: line.before,
            reference: line.reference,
            after: line.after,
        },
    )
}

/// Sources the reader has already been found able to read, each with its mentions and their lines.
async fn sources(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    document: &DocumentRow,
    found: &[rows::LiveSource],
    limits: Limits,
    after: Option<Id>,
) -> ApiResult<Vec<Listed>> {
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Id> = found.iter().map(|source| source.source_id).collect();
    let named = placed(tx, principal, &ids, ctx.now).await?;
    // One more than is listed says whether mentions follow.
    let per_source = i64::try_from(limits.occurrences).unwrap_or(i64::MAX);
    let parts = rows::parts(tx, document.id, found, per_source.saturating_add(1), after).await?;
    // One more than is counted says whether the count reads "or more".
    let counted = i64::try_from(limits.mentions_counted).unwrap_or(i64::MAX);
    let counts = rows::mention_counts(tx, document.id, found, counted.saturating_add(1)).await?;
    let cast = names_for(tx, principal, &parts).await?;
    let names = CastNames(&cast);
    let mut by_source: HashMap<Id, Vec<&rows::PartRow>> = HashMap::new();
    for part in &parts {
        by_source.entry(part.source_id).or_default().push(part);
    }
    let mut out = Vec::new();
    for source in found {
        let Some(place) = named.get(&source.source_id) else {
            continue;
        };
        let Some(all) = by_source.get(&source.source_id) else {
            continue;
        };
        let more = all.len() > limits.occurrences;
        let occurrences: Vec<LinkOccurrence> = all
            .iter()
            .take(limits.occurrences)
            .map(|part| LinkOccurrence {
                anchor: anchor_of(part),
                line: line_of(part, document, &names, limits.line_chars),
                by: cast.author(part.by_user, part.by_thread, part.by_agent.as_deref()),
                at: Time::new(part.at),
            })
            .collect();
        let found_mentions = counts.get(&source.source_id).copied().unwrap_or(0);
        let mut thing = thing_of(
            source,
            place,
            found_mentions.min(counted),
            found_mentions > counted,
            occurrences,
        );
        if more
            && let Some(last) = thing.occurrences.last()
            && let Some(part) = part_of(&last.anchor)
        {
            let of = format!("{}:{}", of_word(thing.kind), source.source_id);
            thing.next_mentions = Some(seal(
                principal,
                document.id,
                LinkDirection::In,
                Some(&of),
                &Position::Mentions { part },
            )?);
        }
        out.push(Listed { thing });
    }
    Ok(out)
}

fn anchor_of(part: &rows::PartRow) -> LinkAnchor {
    match part.part_kind.as_str() {
        "block" => LinkAnchor::Block {
            block_id: part.part_id,
        },
        "comment" => LinkAnchor::Comment {
            comment_id: part.part_id,
            discussion_id: part.discussion_id.unwrap_or(part.part_id),
            block_id: part.block_id,
        },
        "message" => LinkAnchor::Message {
            message_id: part.part_id,
        },
        _ => LinkAnchor::Turn {
            item_id: part.part_id,
            turn: part.turn.map(i64::from),
        },
    }
}

fn thing_of(
    source: &rows::LiveSource,
    place: &Placed,
    mentions: i64,
    mentions_more: bool,
    occurrences: Vec<LinkOccurrence>,
) -> LinkedThing {
    LinkedThing {
        kind: match kind_of(&source.kind) {
            orch_api_contract::search::RelatedKind::Document => LinkedKind::Document,
            orch_api_contract::search::RelatedKind::Chat => LinkedKind::Chat,
            orch_api_contract::search::RelatedKind::Thread => LinkedKind::Thread,
        },
        id: source.source_id,
        path: None,
        node_kind: None,
        name: Some(place.name.clone()),
        label: None,
        readable: true,
        context: place.context.clone(),
        mentions,
        mentions_more,
        occurrences,
        next_mentions: None,
    }
}

/// How the edge store names the kind of thing a reference leads to.
fn target_kind(word: &str) -> Option<InlineRefKind> {
    Some(match word {
        "document" => InlineRefKind::Document,
        "node" => InlineRefKind::Node,
        "chat" => InlineRefKind::Chat,
        "file" => InlineRefKind::File,
        "environment_file" => InlineRefKind::EnvironmentFile,
        _ => return None,
    })
}

/// A chip of a block or comment, as its author wrote it: the reference, its label and its path.
struct Written {
    kind: InlineRefKind,
    id: Id,
    path: Option<String>,
    label: Option<String>,
}

fn written_in(body: &orch_types::Json) -> Vec<Written> {
    fn runs(runs: &[Inline], out: &mut Vec<Written>) {
        for run in runs {
            if let Inline::Ref {
                kind,
                id,
                path,
                label,
                ..
            } = run
            {
                out.push(Written {
                    kind: *kind,
                    id: *id,
                    path: path.clone(),
                    label: label.clone(),
                });
            }
        }
    }
    let mut out = Vec::new();
    match orch_documents::blocks::content_from_json(body) {
        BlockContent::Inline(found) => runs(&found, &mut out),
        BlockContent::Rows(rows) => {
            for cell in rows.iter().flat_map(|row| row.cells.iter()) {
                runs(&cell.content, &mut out);
            }
        }
    }
    out
}

async fn outgoing(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    document: &DocumentRow,
    query: &DocumentReferencesListQuery,
    limits: Limits,
) -> ApiResult<Page<LinkedThing>> {
    let after = match query
        .cursor
        .as_deref()
        .map(|text| open(principal, document.id, LinkDirection::Out, None, text))
        .transpose()?
    {
        None => None,
        Some(Position::TargetKeys { id, kind, key }) => Some(rows::OutPosition {
            target_id: id,
            target_kind: kind,
            path_key: URL_SAFE_NO_PAD.decode(key).map_err(|_| wrong_cursor())?,
        }),
        Some(_) => return Err(wrong_cursor()),
    };
    let limit = limits.page(query.limit);
    // One more than the page says whether targets follow it.
    let mut edges = rows::out_edges(
        tx,
        document.id,
        after.as_ref(),
        i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1),
    )
    .await?;
    let more = edges.len() > limit;
    edges.truncate(limit);
    // The page ends where its last target is, whether or not every target of it can be shown.
    let last = edges.last().map(|edge| {
        (
            edge.target_id,
            edge.target_kind.clone(),
            URL_SAFE_NO_PAD.encode(&edge.target_path_key),
        )
    });
    let first_parts: Vec<Id> = edges.iter().map(|edge| edge.first_part).collect();
    let bodies: HashMap<Id, orch_types::Json> = rows::part_bodies(tx, document.id, &first_parts)
        .await?
        .into_iter()
        .map(|part| (part.part_id, part.body))
        .collect();
    // What each edge was written as: its label, and for a workspace file its path.
    struct Seen {
        edge: rows::OutEdge,
        kind: InlineRefKind,
        path: Option<String>,
        label: Option<String>,
    }
    let mut seen = Vec::new();
    for edge in edges {
        let Some(kind) = target_kind(&edge.target_kind) else {
            continue;
        };
        let written = bodies
            .get(&edge.first_part)
            .map(written_in)
            .unwrap_or_default();
        let wanted_key = edge.target_path_key.clone();
        let hit = written.into_iter().find(|chip| {
            chip.kind == kind
                && chip.id == edge.target_id
                && (kind != InlineRefKind::EnvironmentFile
                    || chip
                        .path
                        .as_deref()
                        .is_some_and(|path| rows::path_key(path) == wanted_key))
        });
        let (path, label) = hit.map_or((None, None), |chip| (chip.path, chip.label));
        // A workspace file whose path was not written as a chip cannot be shown as one.
        if kind == InlineRefKind::EnvironmentFile && path.is_none() {
            continue;
        }
        seen.push(Seen {
            edge,
            kind,
            path,
            label,
        });
    }
    let mut wanted = Wanted::default();
    for each in &seen {
        match each.kind {
            InlineRefKind::Node => wanted.node(each.edge.target_id),
            InlineRefKind::Document => wanted.document(each.edge.target_id),
            InlineRefKind::Chat => wanted.chat(each.edge.target_id),
            InlineRefKind::File => wanted.file(each.edge.target_id),
            InlineRefKind::EnvironmentFile => wanted.environment(each.edge.target_id),
        }
    }
    let cast = Cast::load(tx, principal, wanted, Policy::default()).await?;
    // Whether the reader may read a document, a chat or a place is the exact check of any list, not a name's presence.
    let whole: Vec<rows::LiveSource> = seen
        .iter()
        .filter_map(|each| {
            let kind = match each.kind {
                InlineRefKind::Document => "document",
                InlineRefKind::Chat => "chat",
                InlineRefKind::Node => "conversation",
                _ => return None,
            };
            Some(rows::LiveSource {
                kind: kind.to_owned(),
                source_id: each.edge.target_id,
            })
        })
        .collect();
    let readable: HashSet<Id> = orch_engine::links::readable_by_person(tx, principal, &whole)
        .await
        .map_err(ApiError::internal)?;
    let readable_ids: Vec<Id> = readable.iter().copied().collect();
    let named = placed(tx, principal, &readable_ids, ctx.now).await?;
    let mut items = Vec::new();
    for each in seen {
        let id = each.edge.target_id;
        let (kind, node_kind, name, readable) = match each.kind {
            InlineRefKind::Document => (
                LinkedKind::Document,
                None,
                named.get(&id).map(|place| place.name.clone()),
                readable.contains(&id),
            ),
            InlineRefKind::Chat => (
                LinkedKind::Chat,
                None,
                named.get(&id).map(|place| place.name.clone()),
                readable.contains(&id),
            ),
            InlineRefKind::Node => {
                let node = cast
                    .node(id)
                    .filter(|node| readable.contains(&id) && !node.redacted);
                match node {
                    Some(node) if node.kind == NodeKind::Thread => {
                        (LinkedKind::Thread, Some(NodeKind::Thread), node.name, true)
                    }
                    Some(node) => (LinkedKind::Place, Some(node.kind), node.name, true),
                    None => (LinkedKind::Place, None, None, false),
                }
            }
            InlineRefKind::File => {
                let name = cast.file_of(id);
                (LinkedKind::File, None, name.clone(), name.is_some())
            }
            InlineRefKind::EnvironmentFile => {
                let name = each
                    .path
                    .as_deref()
                    .and_then(|path| cast.environment_file_of(id, path));
                (
                    LinkedKind::EnvironmentFile,
                    None,
                    name.clone(),
                    name.is_some(),
                )
            }
        };
        let name = if readable { name } else { None };
        items.push(LinkedThing {
            kind,
            id,
            path: each.path,
            node_kind,
            name,
            label: each.label,
            readable,
            context: if readable {
                named
                    .get(&id)
                    .map(|place| place.context.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            mentions: each.edge.mentions,
            mentions_more: false,
            occurrences: Vec::new(),
            next_mentions: None,
        });
    }
    let next_cursor = match (more, last) {
        (true, Some((id, kind, key))) => Some(seal(
            principal,
            document.id,
            LinkDirection::Out,
            None,
            &Position::TargetKeys { id, kind, key },
        )?),
        _ => None,
    };
    Ok(Page { items, next_cursor })
}
