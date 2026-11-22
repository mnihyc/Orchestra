//! What refers to a document, with the line around each mention, and what the document refers to.

use super::placed::CastNames;

use super::placed::Placed;

use super::placed::kind_of;

use super::placed::placed;

use super::placed::store_kind;

use super::Limits;

use super::Position;

use super::live_document;

use super::open as open_native;

use super::seal as seal_native;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::documents::build::Cast;

use crate::routes::documents::build::Wanted;

use crate::routes::nodes::invalid;

use base64::Engine as _;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use orch_api_contract::common::Page;

use orch_api_contract::documents::BlockContent;

use orch_api_contract::documents::Inline;

use orch_api_contract::documents::InlineRefKind;

use orch_api_contract::links::DocumentReferencesListQuery;

use orch_api_contract::links::LinkAnchor;

use orch_api_contract::links::LinkDirection;

use orch_api_contract::links::LinkLine;

use orch_api_contract::links::LinkOccurrence;

use orch_api_contract::links::LinkedKind;

use orch_api_contract::links::LinkedThing;

use orch_api_contract::nodes::NodeKind;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::documents::DocumentRow;

use orch_db::queries::links as rows;

use orch_db::Conn;

use orch_db::DbError;

use orch_documents::links;

use orch_documents::links::Target;

use orch_engine::links::Person;

use orch_engine::links::SourcePageAsk;

use orch_types::Id;

use orch_types::Time;

use std::collections::HashMap;

use std::collections::HashSet;
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
    if query.limit == Some(0) {
        return Err(invalid("limit", "must be greater than zero"));
    }
    if !query_text(&query).is_empty()
        && orch_db::queries::search::Query::parse(query_text(&query))
            .terms()
            .is_empty()
    {
        return Err(invalid("q", "give at least one word or quoted phrase"));
    }
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

fn query_text(query: &DocumentReferencesListQuery) -> &str {
    query.q.as_deref().unwrap_or("").trim()
}

fn seal_list(
    principal: &Principal,
    document: Id,
    direction: LinkDirection,
    of: Option<&str>,
    position: &Position,
    query: &str,
) -> ApiResult<String> {
    seal_native(
        principal,
        document,
        direction,
        of,
        &Position::Filtered {
            query: query.to_owned(),
            position: Box::new(position.clone()),
        },
    )
}

fn open_list(
    principal: &Principal,
    document: Id,
    direction: LinkDirection,
    of: Option<&str>,
    text: &str,
    query: &str,
) -> ApiResult<Position> {
    match open_native(principal, document, direction, of, text)? {
        Position::Filtered {
            query: expected,
            position,
        } if expected == query => Ok(*position),
        _ => Err(wrong_cursor()),
    }
}

fn matches_name(words: &orch_db::queries::search::Query, row: &LinkedThing) -> bool {
    words.matches(
        &[
            row.name.as_deref().unwrap_or(""),
            row.label.as_deref().unwrap_or(""),
            row.path.as_deref().unwrap_or(""),
            &row.context
                .iter()
                .filter_map(|node| node.name.as_deref())
                .collect::<Vec<_>>()
                .join(" "),
        ]
        .join(" "),
    )
}

/// The conservative scopes a person's sources are narrowed by before each is checked exactly.
async fn scopes_of(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
) -> ApiResult<orch_types::Json> {
    Ok(
        orch_authz::everything::project_related(conn, principal, document)
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
    let text = query_text(query);
    let words = orch_db::queries::search::Query::parse(text);
    let mut before = match query
        .cursor
        .as_deref()
        .map(|cursor| {
            open_list(
                principal,
                document.id,
                LinkDirection::In,
                None,
                cursor,
                text,
            )
        })
        .transpose()?
    {
        None => None,
        Some(Position::Sources { below }) => Some(below),
        Some(_) => return Err(wrong_cursor()),
    };
    let scopes = scopes_of(tx, principal, document.id).await?;
    let wanted = limits.page(query.limit);
    let batch = if text.is_empty() {
        wanted
    } else {
        wanted.max(limits.page_size)
    };
    let mut remaining = limits.access_checks;
    let mut items = Vec::new();
    let next;
    loop {
        let page = orch_engine::links::sources_page(
            tx,
            &Person(principal),
            SourcePageAsk {
                target: document.id,
                scopes: &scopes,
                before,
                limit: batch.min(remaining).max(1),
                checks: remaining,
            },
        )
        .await
        .map_err(ApiError::internal)?;
        remaining = remaining.saturating_sub(page.examined);
        let listed = sources(
            ctx,
            tx,
            principal,
            document,
            &page.sources,
            limits,
            (None, ""),
        )
        .await?;
        let mut extra = false;
        for row in listed.into_iter().map(|row| row.thing) {
            if !text.is_empty() && !matches_name(&words, &row) {
                continue;
            }
            if items.len() == wanted {
                extra = true;
                break;
            }
            items.push(row);
        }
        if extra {
            next = items.last().map(|row: &LinkedThing| row.id);
            break;
        }
        if page.next.is_none() || remaining == 0 || text.is_empty() || items.len() == wanted {
            next = page.next;
            break;
        }
        before = page.next;
    }
    let next_cursor = next
        .map(|below| {
            seal_list(
                principal,
                document.id,
                LinkDirection::In,
                None,
                &Position::Sources { below },
                text,
            )
        })
        .transpose()?;
    Ok(Page { items, next_cursor })
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
        .map(|text| {
            open_list(
                principal,
                document.id,
                LinkDirection::In,
                Some(of),
                text,
                query_text(query),
            )
        })
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
    let wanted = query
        .limit
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(limits.occurrences);
    let mention_limits = Limits {
        occurrences: wanted.min(limits.access_checks).max(1),
        ..limits
    };
    let mut listed = sources(
        ctx,
        tx,
        principal,
        document,
        std::slice::from_ref(&source),
        mention_limits,
        (after, query_text(query)),
    )
    .await?;
    let text = query_text(query);
    if text.is_empty() {
        let next_cursor = listed
            .first()
            .and_then(|row| row.thing.next_mentions.clone());
        return Ok(Page {
            items: listed.into_iter().map(|row| row.thing).collect(),
            next_cursor,
        });
    }
    let Some(first) = listed.pop() else {
        return Ok(Page::last(Vec::new()));
    };
    let mut thing = first.thing;
    let words = orch_db::queries::search::Query::parse(text);
    let mut cursor = thing
        .occurrences
        .last()
        .and_then(|row| part_of(&row.anchor));
    let mut more = thing.next_mentions.is_some();
    let mut examined = thing.occurrences.len();
    let mut matches = thing
        .occurrences
        .drain(..)
        .filter(|row| matches_line(&words, &row.line))
        .collect::<Vec<_>>();
    while more && matches.len() <= wanted && examined < limits.access_checks {
        let batch = limits
            .page_size
            .max(wanted)
            .min(limits.access_checks - examined)
            .max(1);
        let mut parts = rows::parts(
            tx,
            document.id,
            std::slice::from_ref(&source),
            i64::try_from(batch)
                .map_err(ApiError::internal)?
                .saturating_add(1),
            cursor,
        )
        .await?;
        more = parts.len() > batch;
        parts.truncate(batch);
        let cast = names_for(tx, principal, &parts).await?;
        let names = CastNames(&cast);
        examined += parts.len();
        cursor = parts.last().map(|part| part.part_id);
        for part in parts {
            let line = line_of(&part, document, &names, limits.line_chars);
            if matches_line(&words, &line) {
                matches.push(LinkOccurrence {
                    anchor: anchor_of(&part),
                    line,
                    by: cast.author(part.by_user, part.by_thread, part.by_agent.as_deref()),
                    at: Time::new(part.at),
                });
            }
        }
    }
    if matches.len() > wanted {
        matches.truncate(wanted);
        cursor = matches.last().and_then(|row| part_of(&row.anchor));
        more = true;
    }
    let next_cursor = if more {
        cursor
            .map(|part| {
                seal_list(
                    principal,
                    document.id,
                    LinkDirection::In,
                    Some(of),
                    &Position::Mentions { part },
                    text,
                )
            })
            .transpose()?
    } else {
        None
    };
    thing.next_mentions = next_cursor.clone();
    thing.occurrences = matches;
    Ok(Page {
        items: if thing.occurrences.is_empty() {
            Vec::new()
        } else {
            vec![thing]
        },
        next_cursor,
    })
}

fn matches_line(words: &orch_db::queries::search::Query, line: &LinkLine) -> bool {
    words.matches(&format!("{}{}{}", line.before, line.reference, line.after))
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
    mention: (Option<Id>, &str),
) -> ApiResult<Vec<Listed>> {
    let (after, mention_query) = mention;
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
            thing.next_mentions = Some(seal_list(
                principal,
                document.id,
                LinkDirection::In,
                Some(&of),
                &Position::Mentions { part },
                mention_query,
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
    let text = query_text(query);
    if text.is_empty() {
        return outgoing_page(ctx, tx, principal, document, query, limits)
            .await
            .map(|(page, _)| page);
    }
    let words = orch_db::queries::search::Query::parse(text);
    let wanted = limits.page(query.limit);
    let batch = wanted.max(limits.page_size);
    let mut remaining = limits.access_checks;
    let mut scan = query.clone();
    let mut items = Vec::new();
    let next_cursor;
    loop {
        scan.limit = Some(u32::try_from(batch.min(remaining).max(1)).map_err(ApiError::internal)?);
        let (page, examined) = outgoing_page(ctx, tx, principal, document, &scan, limits).await?;
        remaining = remaining.saturating_sub(examined);
        let mut extra = false;
        for row in page.items {
            if !matches_name(&words, &row) {
                continue;
            }
            if items.len() == wanted {
                extra = true;
                break;
            }
            items.push(row);
        }
        if extra {
            next_cursor = items
                .last()
                .map(|row| {
                    let kind = match row.kind {
                        LinkedKind::Document => "document",
                        LinkedKind::Chat => "chat",
                        LinkedKind::Thread | LinkedKind::Place => "node",
                        LinkedKind::File => "file",
                        LinkedKind::EnvironmentFile => "environment_file",
                    }
                    .to_owned();
                    let key = row
                        .path
                        .as_deref()
                        .map(|path| URL_SAFE_NO_PAD.encode(rows::path_key(path)))
                        .unwrap_or_default();
                    seal_list(
                        principal,
                        document.id,
                        LinkDirection::Out,
                        None,
                        &Position::TargetKeys {
                            id: row.id,
                            kind,
                            key,
                        },
                        text,
                    )
                })
                .transpose()?;
            break;
        }
        if page.next_cursor.is_none() || remaining == 0 || examined == 0 || items.len() == wanted {
            next_cursor = page.next_cursor;
            break;
        }
        scan.cursor = page.next_cursor;
    }
    Ok(Page { items, next_cursor })
}

async fn outgoing_page(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    document: &DocumentRow,
    query: &DocumentReferencesListQuery,
    limits: Limits,
) -> ApiResult<(Page<LinkedThing>, usize)> {
    let after = match query
        .cursor
        .as_deref()
        .map(|text| {
            open_list(
                principal,
                document.id,
                LinkDirection::Out,
                None,
                text,
                query_text(query),
            )
        })
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
    let examined = edges.len();
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
        (true, Some((id, kind, key))) => Some(seal_list(
            principal,
            document.id,
            LinkDirection::Out,
            None,
            &Position::TargetKeys { id, kind, key },
            query_text(query),
        )?),
        _ => None,
    };
    Ok((Page { items, next_cursor }, examined))
}
