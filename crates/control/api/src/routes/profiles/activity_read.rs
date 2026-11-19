//! Native profile work is cut at its actual thread sources before totals and recent ranking.

use super::open_person;
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::view;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{NodePermission, ObjectKind, PermissionName};
use orch_api_contract::account_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_api_contract::users::{ProfileActivity, ProfileActivityDay, ProfileActivityRecent};
use orch_authz::engine::Graph;
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::identity;
use orch_db::usage::rollup;
use orch_types::{Id, Time};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

// Keep the native calendar span and recent-topic window.
const ACTIVITY_DAYS: i64 = 53 * 7;
const RECENT_MAX: usize = 8;
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
const TOPIC: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Topic);

pub(super) struct Activity;

pub(super) struct Selection {
    account: Id,
    from: NaiveDate,
    to: NaiveDate,
    rows: Vec<rollup::ProfileDay>,
    recent: Vec<(Id, DateTime<Utc>)>,
}

fn collection(
    source: Source,
    family: KeyTargetKind,
    values: Vec<KeyResolvedTarget>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        if values.is_empty() {
            Vec::new()
        } else {
            vec![(family, values)]
        },
    )
}

fn node_targets(
    graph: &Graph,
    ids: impl IntoIterator<Item = Id>,
    family: KeyTargetKind,
) -> BTreeMap<Id, KeyResolvedTarget> {
    ids.into_iter()
        .filter_map(|id| {
            orch_engine::operation_scope::targets::from_node_identity(graph, id)
                .filter(|target| target.object.kind == family)
                .map(|target| (id, target))
        })
        .collect()
}

impl native::Selected for Activity {
    type Source = Source;
    type Query = ();
    type Value = Selection;
    type Reply = ProfileActivity;
    const DECLARATION: &'static Declaration<Source> = &sources::PROFILE_ACTIVITY;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let person = identity::user_by_username(conn, ctx.param("username")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let account = person.id;
            let root = native::Facts::one(
                Source::ProfileAccount,
                ACCOUNT,
                targets::account(conn, account).await?,
            );
            let plan = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let opened = open_person(conn, &principal, person).await?;
            if !opened.page.show_activity && principal.user() != account {
                return Err(ApiError::not_found());
            }
            let to = ctx.now.date_naive();
            let from = to - Duration::days(ACTIVITY_DAYS - 1);
            let rows = rollup::profile_days(conn, account, from, to).await?;
            let ids: BTreeSet<_> = rows
                .iter()
                .flat_map(|row| [row.thread_id, row.topic_id])
                .collect();
            let graph = Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let mut threads = node_targets(&graph, rows.iter().map(|row| row.thread_id), THREAD);
            let mut topics = node_targets(&graph, rows.iter().map(|row| row.topic_id), TOPIC);
            if native::modern(caller) {
                let permits = |source, usage, family, target: &KeyResolvedTarget| {
                    native::permits_usage(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &source,
                        usage,
                        family,
                        target,
                    )
                };
                let mut allowed = BTreeMap::new();
                for (id, target) in threads {
                    if permits(
                        Source::ActivityThreads,
                        FacetUsage::Required,
                        THREAD,
                        &target,
                    )? {
                        allowed.insert(id, target);
                    }
                }
                threads = allowed;
                let mut allowed = BTreeMap::new();
                for (id, target) in topics {
                    if permits(
                        Source::ActivityTopics,
                        FacetUsage::ResponseContent,
                        TOPIC,
                        &target,
                    )? {
                        allowed.insert(id, target);
                    }
                }
                topics = allowed;
            }
            let facts = vec![
                root.clone(),
                collection(
                    Source::ActivityThreads,
                    THREAD,
                    threads.values().cloned().collect(),
                ),
                collection(
                    Source::ActivityTopics,
                    TOPIC,
                    topics.values().cloned().collect(),
                ),
            ];
            let known = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let thread_ids: Vec<_> = threads.keys().copied().collect();
            let visible: HashSet<_> = nodes::holding(
                conn,
                &principal,
                &thread_ids,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|each| each.id)
            .collect();
            let rows: Vec<_> = rows
                .into_iter()
                .filter(|row| visible.contains(&row.thread_id))
                .collect();
            let selected_threads: BTreeSet<_> = rows.iter().map(|row| row.thread_id).collect();
            let mut recent = HashMap::new();
            for row in &rows {
                recent
                    .entry(row.topic_id)
                    .and_modify(|at: &mut DateTime<Utc>| *at = (*at).max(row.last_at))
                    .or_insert(row.last_at);
            }
            let topic_ids: Vec<_> = recent
                .keys()
                .filter(|id| topics.contains_key(*id))
                .copied()
                .collect();
            let visible_topics: HashSet<_> = nodes::holding(
                conn,
                &principal,
                &topic_ids,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|each| each.id)
            .collect();
            let mut recent: Vec<_> = recent
                .into_iter()
                .filter(|(id, _)| visible_topics.contains(id))
                .collect();
            recent.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            recent.truncate(RECENT_MAX);
            let facts = vec![
                root,
                collection(
                    Source::ActivityThreads,
                    THREAD,
                    selected_threads
                        .iter()
                        .filter_map(|id| threads.remove(id))
                        .collect(),
                ),
                collection(
                    Source::ActivityTopics,
                    TOPIC,
                    recent
                        .iter()
                        .filter_map(|(id, _)| topics.remove(id))
                        .collect(),
                ),
            ];
            Ok((
                Selection {
                    account,
                    from,
                    to,
                    rows,
                    recent,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the profile reader changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<ProfileActivity>> {
        Box::pin(async move {
            let ids: Vec<_> = value.recent.iter().map(|(id, _)| *id).collect();
            let refs = view::refs(conn, principal, &ids, Policy::default()).await?;
            let recent = value
                .recent
                .iter()
                .filter_map(|(id, at)| {
                    refs.get(id).cloned().map(|node| ProfileActivityRecent {
                        node,
                        last_at: Time::new(*at),
                    })
                })
                .collect();
            let mut days = BTreeMap::<NaiveDate, i64>::new();
            for row in &value.rows {
                *days.entry(row.day).or_default() += row.turns;
            }
            let total = days.values().sum();
            let _ = ctx;
            Ok(ProfileActivity {
                from: value.from.to_string(),
                to: value.to.to_string(),
                total,
                recent,
                days: days
                    .into_iter()
                    .map(|(day, turns)| ProfileActivityDay {
                        day: day.to_string(),
                        turns,
                    })
                    .collect(),
            })
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        opened: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let rows: BTreeSet<_> = fresh
                .rows
                .iter()
                .map(|row| (row.day, row.thread_id, row.topic_id))
                .collect();
            Ok(opened.account == fresh.account
                && opened
                    .rows
                    .iter()
                    .all(|row| rows.contains(&(row.day, row.thread_id, row.topic_id))))
        })
    }
}
