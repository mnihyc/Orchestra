//! Bounded indexed word windows with exact current permission frames and server-side field and occurrence ranking.

use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use crate::routes::nodes::invalid;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_api_contract::{
    common::Page,
    search::{SearchHit, SearchListQuery, SearchSort},
};
use orch_db::queries::search::{self as rows, candidates};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::Time;
use std::collections::{BTreeMap, HashSet};

pub(super) async fn retrieve(
    mut ctx: Ctx,
    query: SearchListQuery,
    shown: Vec<String>,
) -> ApiResult<Page<SearchHit>> {
    let thread_scope = if let Some(scope) = query.scope {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND kind='thread')",
        )
        .bind(scope)
        .fetch_one(ctx.app.db.pool())
        .await
        .map_err(orch_db::DbError::from)?
    } else {
        false
    };
    if query.regex == Some(true)
        && (query.continuation_id.is_some() || query.projection_revision.is_some())
    {
        return Err(invalid(
            "regex",
            "select the active history for an expression",
        ));
    }
    if query.regex != Some(true)
        && (thread_scope || query.continuation_id.is_some() || query.projection_revision.is_some())
    {
        return super::content::search(ctx, query, ()).await;
    }
    let expression = (query.regex == Some(true))
        .then(|| {
            orch_engine::tools::search::search_expression(&query.q)
                .map_err(|reason| invalid("q", &reason))
        })
        .transpose()?;
    let literal = expression
        .as_ref()
        .map(|_| orch_engine::tools::search::expression_literal(&query.q).unwrap_or_default());
    let text = literal.as_deref().unwrap_or_else(|| query.q.trim());
    if expression.is_none() && rows::Query::parse(text).terms().is_empty() {
        return Err(invalid("q", "give a word or phrase"));
    }
    if query.from.zip(query.to).is_some_and(|(a, b)| a > b) {
        return Err(invalid("to", "precedes from"));
    }
    if query.chat_id.is_some() && query.document_id.is_some() {
        return Err(invalid("document_id", "cannot be combined with a chat"));
    }
    let requested = super::kinds(query.kinds.as_deref(), super::content::CONTENT_KINDS)?;
    let item_kinds: Vec<String> = requested
        .iter()
        .filter(|k| !matches!(k.as_str(), "document" | "chat_message"))
        .cloned()
        .collect();
    let mut position = query
        .cursor
        .as_ref()
        .map(|raw| {
            let decoded = URL_SAFE_NO_PAD
                .decode(raw)
                .map_err(|_| invalid("cursor", "is not a word continuation"))?;
            serde_json::from_slice::<candidates::Position>(&decoded)
                .map_err(|_| invalid("cursor", "is not a word continuation"))
        })
        .transpose()?
        .unwrap_or_default();
    let target = usize::try_from(query.limit.unwrap_or_default()).unwrap_or(0);
    let shown: HashSet<String> = shown.into_iter().collect();
    let mut collected = BTreeMap::new();
    let mut next;
    let mut windows = 0_i64;
    loop {
        ctx.now = ctx.app.clock.now();
        let mut tx = ctx.app.db.begin().await?;
        rows::snapshot(&mut tx).await?;
        let mut cancellation = rows::cancel_on_drop(&ctx.app.db, &mut tx).await?;
        let principal = authorization::principal(&ctx, &mut tx).await?;
        let advanced = super::advanced_selection(&mut tx, &principal, &query, ctx.now).await?;
        let place = query.scope.map_or(Place::System, Place::Node);
        let wanted = get::<i64>(&mut tx, place, keys::SEARCH_WORD_CANDIDATES)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("word candidate setting missing"))?;
        let probe = get::<i64>(&mut tx, place, keys::SEARCH_WORD_PROBE_CANDIDATES)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("word probe setting missing"))?;
        let recent = get::<i64>(&mut tx, place, keys::SEARCH_WORD_RECENT_CANDIDATES)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("recent word setting missing"))?;
        let tuning = get::<candidates::Tuning>(&mut tx, place, keys::SEARCH_WORD_TUNING)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("word retrieval setting missing"))?;
        let phase = std::time::Instant::now();
        let scope_projection = if query.document_id.is_some() || query.chat_id.is_some() {
            Some(
                orch_authz::everything::project_search(
                    &mut tx,
                    &principal,
                    query.scope,
                    &query.document_id.into_iter().collect::<Vec<_>>(),
                    &query.chat_id.into_iter().collect::<Vec<_>>(),
                    &[],
                )
                .await
                .map_err(ApiError::internal)?,
            )
        } else {
            orch_authz::everything::project_search_scope(
                &mut tx,
                &principal,
                query.scope,
                tuning.scope_candidates,
            )
            .await
            .map_err(ApiError::internal)?
        };
        tracing::debug!(target: "orch::search::timing", phase="word_scope", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
        let empty = serde_json::json!({"search_subtree": principal.caller().grant().and_then(orch_authz::TokenGrant::subtree),
            "search_readable":scope_projection.as_ref().map(|projection| projection.search_candidates(&principal))
                .unwrap_or_else(|| serde_json::json!({"unrestricted":true})),"search_regex":query.regex==Some(true),
            "search_advanced":advanced});
        let base = rows::Content {
            selected: None,
            items: None,
            continuation: None,
            user: principal.user(),
            groups: principal.groups(),
            member_roots: &[],
            grants: &empty,
            at: ctx.now,
            scope: query.scope,
            chat: query.chat_id,
            document: query.document_id,
            participating: query.participating.unwrap_or(false),
            relevance: query.sort.unwrap_or_default() == SearchSort::Relevance,
            names: (query.sort == Some(SearchSort::Relevance) || query.title_only == Some(true))
                && (query.kinds.is_none()
                    || (query.title_only == Some(true) && !item_kinds.is_empty()))
                && query.author_id.is_none()
                && !query.with_file.unwrap_or(false)
                && !query.mentions_me.unwrap_or(false)
                && query.chat_id.is_none()
                && query.document_id.is_none(),
            with_file: query.with_file.unwrap_or(false),
            mentions_me: query
                .mentions_me
                .unwrap_or(false)
                .then_some(principal.user()),
            text,
            item_kinds: &item_kinds,
            chat_messages: requested.iter().any(|k| k == "chat_message"),
            document_content: requested.iter().any(|k| k == "document"),
            author: query.author_id,
            from: query.from.map(Time::get),
            to: query.to.map(Time::get),
            after: None,
            limit: i64::MAX,
        };
        let phase = std::time::Instant::now();
        let window = candidates::window_with_tuning(
            &mut tx, &base, position, wanted, probe, recent, &tuning,
        )
        .await?;
        tracing::debug!(target: "orch::search::timing", phase="word_candidates", elapsed_ms=phase.elapsed().as_secs_f64()*1000., parts=window.parts.len());
        next = window.next;
        windows = windows.saturating_add(1);
        if window.parts.is_empty() {
            cancellation.disarm();
            tx.commit().await.map_err(orch_db::DbError::from)?;
            if next.is_none() || collected.len() >= target || windows >= tuning.request_windows() {
                break;
            }
            position = next
                .take()
                .ok_or_else(|| ApiError::internal("word continuation vanished"))?;
            continue;
        }
        let selected = rows::CandidateSet::of(&window.parts);
        let phase = std::time::Instant::now();
        let projection = orch_authz::everything::project_search(
            &mut tx,
            &principal,
            query.scope,
            &selected.documents,
            &selected.chats,
            &selected.nodes,
        )
        .await
        .map_err(ApiError::internal)?;
        tracing::debug!(target: "orch::search::timing", phase="word_access", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
        let mut grants = projection.grants.clone();
        grants["search_regex"] = serde_json::json!(query.regex == Some(true));
        grants["search_advanced"] = empty["search_advanced"].clone();
        let ask = rows::Content {
            selected: Some(&selected),
            member_roots: &projection.member_roots,
            grants: &grants,
            ..base
        };
        let phase = std::time::Instant::now();
        let mut found = rows::content(&mut tx, &ask).await?;
        tracing::debug!(target: "orch::search::timing", phase="word_rows", elapsed_ms=phase.elapsed().as_secs_f64()*1000., parts=found.len());
        if let Some(expression) = &expression {
            found.retain(|row| expression.is_match(&row.text));
            for row in &mut found {
                row.matches = if ask.relevance {
                    i64::try_from(expression.find_iter(&row.text).count()).unwrap_or(i64::MAX)
                } else {
                    0
                };
            }
        }
        let phase = std::time::Instant::now();
        let hits = super::content::hydrate(&mut tx, &principal, &found, &query, ctx.now).await?;
        tracing::debug!(target: "orch::search::timing", phase="word_payloads", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
        for (row, hit) in found.into_iter().zip(hits) {
            let key = super::stream::identity(&hit);
            if shown.contains(&key) {
                continue;
            }
            let rank = (
                row.relevance,
                row.matches,
                row.at,
                row.id,
                row.kind,
                row.block_id.is_some(),
            );
            match collected.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((rank, hit));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if rank > entry.get().0 {
                        entry.insert((rank, hit));
                    }
                }
            }
        }
        cancellation.disarm();
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if collected.len() >= target || next.is_none() || windows >= tuning.request_windows() {
            break;
        }
        position = next
            .take()
            .ok_or_else(|| ApiError::internal("word continuation vanished"))?;
    }
    let next_cursor = next
        .map(|position| {
            serde_json::to_vec(&position)
                .map(|value| URL_SAFE_NO_PAD.encode(value))
                .map_err(ApiError::internal)
        })
        .transpose()?;
    let mut ranked: Vec<_> = collected.into_values().collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(Page {
        items: ranked.into_iter().map(|(_, hit)| hit).collect(),
        next_cursor,
    })
}
