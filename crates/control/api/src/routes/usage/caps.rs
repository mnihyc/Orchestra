//! Caps made, changed and removed by whoever the place of the cap says; how each of them stands; everything that bounds work at a node; a person's own share of the caps counted per person; and who waits for room in a cap.
//!
//! **Where a cap is defined decides who may touch it** ([`orch_authz::caps`]):
//! `usage.manage` on its node, `provider.manage` on its provider (and sight of
//! the node it also names), or a system admin for a cap of the system. Reading
//! the caps defined in a place needs `usage.view`, `provider.view`, or a system
//! admin. A person a cap covers reads their own share of it without any of
//! these, in their own limits, in the limits of a place they can see, and in the
//! queue of a cap on turns running at once.
//!
//! **A cap is checked whole before anything is written**: its subject and split
//! fit each other, a cap on turns at once has no period, warning, `at_limit` or
//! `uncounted`, the amount is a whole number where the meter counts whole
//! things and has at most six decimal places in credits, and a group belongs to
//! the tree of the place, and a model it names as `@<group>` names a group that can exist. A cap of the
//! same node, provider, subject, split, models, tiers, harness kinds, meter and period is
//! `409 conflict.exists`; a change
//! against a revision that is out of date is `409 conflict.stale_revision`
//! with the cap as it stands.
//!
//! **A cap made, changed or removed is looked at by the engine at once**: the
//! turns it covers that are under way are asked of it as when their usage is
//! reported, so that a cap that is reached and says `stop` ends them, and the
//! threads and inputs that waited are let through in order when room was made.
//! A person who is covered by a cap and is not in the tree of its place is still
//! covered.

use super::names::Names;
use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::nodes::{actor_name, invalid};
use crate::sign_in::origin;
use chrono::{DateTime, NaiveDate, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_agent_contract::discovery::ModelPattern;
use orch_api_contract::access::Role;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::Group;
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::resources::is_model_group_name;
use orch_api_contract::usage::{
    CAP_CREDIT_DECIMALS, CAP_WARN_PERCENT_MAX, CAP_WARN_PERCENT_MIN, CapAtLimit, CapBypassLast,
    CapBypasses, CapInput, CapLeftOut, CapMeter, CapPeriod, CapSplit, CapState, CapStatus,
    CapSubject, CapSubjectInput, CapUncounted, Limits, UsageCap, UsageCapListQuery,
    UsageCapUpdateBody, UsageCapWaiting, UsageCapWaitingListReply, UsageLimitsGetQuery,
};
use orch_authz::caps::{Home, Verdict, check_manage, check_view, may_pass};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::{access, threads};
use orch_db::usage::caps::{
    self as rows, CapRow, Change, DefinedOn, NewCap, Updated, at_limit, meter, period, split,
    subject, uncounted,
};
use orch_db::usage::micros::Micros;
use orch_db::usage::rollup;
use orch_engine::caps as engine;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};

/// The longest note a cap carries, as the table has it.
const NOTE_MAX_CHARS: usize = 500;
/// The longest model pattern or harness kind a cap names: far more than any model is called.
const WORD_MAX_CHARS: usize = 200;
/// How many model patterns, and how many harness kinds, a cap names at most: a cap is a rule, not a list.
const WORDS_MAX: usize = 50;

// ---------------------------------------------------------------- words of the contract and of the table

fn word<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

const fn meter_word(value: CapMeter) -> &'static str {
    match value {
        CapMeter::Tokens => meter::TOKENS,
        CapMeter::Credits => meter::CREDITS,
        CapMeter::Running => meter::RUNNING,
    }
}

const fn split_word(value: CapSplit) -> &'static str {
    match value {
        CapSplit::Shared => split::SHARED,
        CapSplit::Each => split::EACH,
    }
}

const fn period_word(value: CapPeriod) -> &'static str {
    match value {
        CapPeriod::Day => period::DAY,
        CapPeriod::Week => period::WEEK,
        CapPeriod::Month => period::MONTH,
        CapPeriod::Total => period::TOTAL,
    }
}

const fn at_limit_word(value: CapAtLimit) -> &'static str {
    match value {
        CapAtLimit::FinishTurn => at_limit::FINISH_TURN,
        CapAtLimit::Stop => at_limit::STOP,
    }
}

const fn uncounted_word(value: CapUncounted) -> &'static str {
    match value {
        CapUncounted::Allow => uncounted::ALLOW,
        CapUncounted::Block => uncounted::BLOCK,
    }
}

/// What a meter counts, in a sentence.
fn counts(meter_name: &str) -> &'static str {
    match meter_name {
        meter::TOKENS => "tokens",
        meter::CREDITS => "credits",
        _ => "turns at once",
    }
}

// ---------------------------------------------------------------- who may

/// What is done to a cap, which decides who may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Make, change or remove it.
    Manage,
    /// Read it and its use.
    View,
}

/// Refuses the caller who may not do `need` to a cap defined in `home`, with what they lack.
async fn require(conn: &mut Conn, principal: &Principal, home: Home, need: Need) -> ApiResult<()> {
    let verdict = match need {
        Need::Manage => check_manage(conn, principal, home, Policy::default()).await,
        Need::View => check_view(conn, principal, home, Policy::default()).await,
    };
    match verdict {
        Ok(Verdict::Allowed) => Ok(()),
        Ok(Verdict::Refused(refusal)) => Err(refusals::denied(conn, *refusal).await?),
        Ok(Verdict::SystemAdminOnly) => Err(refusals::system_admin_only(match need {
            Need::Manage => "only a system admin changes the caps of the system",
            Need::View => "only a system admin reads the caps of the system",
        })),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

/// Whether the caller may do `need` to a cap defined in `home`; a place that is archived or gone, or a caller
/// who is suspended, may not.
async fn may(conn: &mut Conn, principal: &Principal, home: Home, need: Need) -> ApiResult<bool> {
    let verdict = match need {
        Need::Manage => check_manage(conn, principal, home, Policy::default()).await,
        Need::View => check_view(conn, principal, home, Policy::default()).await,
    };
    match verdict {
        Ok(verdict) => Ok(verdict.is_allowed()),
        Err(
            orch_authz::Error::Suspended { .. }
            | orch_authz::Error::Archived { .. }
            | orch_authz::Error::NotFound(_),
        ) => Ok(false),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

// ---------------------------------------------------------------- what is shown of a cap

/// What a set of caps is named by: the people, places, providers and groups they and their use name.
struct Known {
    names: Names,
    groups: HashMap<Id, Group>,
}

impl Known {
    /// Names what `caps` name, and the people in `more_users`.
    async fn load(
        conn: &mut Conn,
        principal: &Principal,
        caps: &[&CapRow],
        more_users: &[Id],
    ) -> ApiResult<Self> {
        let mut users: Vec<Id> = more_users.to_vec();
        let (mut providers, mut nodes, mut groups) = (Vec::new(), Vec::new(), Vec::new());
        for cap in caps {
            users.push(cap.created_by);
            match (cap.subject_kind.as_str(), cap.subject_id) {
                (subject::USER, Some(id)) => users.push(id),
                (subject::GROUP, Some(id)) => groups.push(id),
                (subject::ROLE | subject::MEMBERS, Some(id)) => nodes.push(id),
                _ => {}
            }
            providers.extend(cap.provider_id);
            nodes.extend(cap.node_id);
        }
        for list in [&mut users, &mut providers, &mut nodes, &mut groups] {
            list.sort_unstable();
            list.dedup();
        }
        let names = Names::load(conn, principal, &users, &providers, &nodes).await?;
        let groups = if groups.is_empty() {
            HashMap::new()
        } else {
            let rows = access::groups_full(conn, &groups).await?;
            crate::routes::directory::groups_named(conn, principal, rows)
                .await?
                .into_iter()
                .map(|group| (group.id, group))
                .collect()
        };
        Ok(Self { names, groups })
    }

    /// A cap as the contract has it.
    fn cap(&self, cap: &CapRow, can_edit: bool) -> ApiResult<UsageCap> {
        let bad = |what: &str| {
            ApiError::internal(format!(
                "a cap holds {what} that is not a word of the contract"
            ))
        };
        let subject = match (cap.subject_kind.as_str(), cap.subject_id) {
            (subject::MEMBERS, Some(id)) => CapSubject::Members {
                node: self.names.node(id, NodeKind::Org),
            },
            (subject::USER, Some(id)) => CapSubject::User {
                user: self.names.user(id),
            },
            (subject::GROUP, Some(id)) => CapSubject::Group {
                group: self.groups.get(&id).cloned().unwrap_or_else(|| Group {
                    id,
                    org_id: Id::nil(),
                    place: Some("a removed place".to_owned()),
                    name: "(removed)".to_owned(),
                    description: String::new(),
                    member_count: 0,
                }),
            },
            (subject::ROLE, _) => CapSubject::Role {
                at: cap.subject_id.map(|id| self.names.node(id, NodeKind::Org)),
                role: cap
                    .subject_role
                    .as_deref()
                    .and_then(word::<Role>)
                    .ok_or_else(|| bad("a role"))?,
            },
            _ => CapSubject::Everyone,
        };
        Ok(UsageCap {
            id: cap.id,
            node: cap.node_id.map(|id| self.names.node(id, NodeKind::Org)),
            provider: cap.provider_id.and_then(|id| self.names.provider(id)),
            subject,
            split: word(&cap.split).ok_or_else(|| bad("a split"))?,
            models: cap.models.clone(),
            tiers: cap.tiers.clone(),
            harnesses: cap
                .harness_kinds
                .iter()
                .map(|kind| crate::programs::shipped_name(kind, None))
                .collect(),
            harness_kinds: cap.harness_kinds.clone(),
            meter: word(&cap.meter).ok_or_else(|| bad("a meter"))?,
            period: cap.period.as_deref().and_then(word),
            counted_from: cap.counted_from.map(|day| day.to_string()),
            amount: cap
                .amount
                .as_deref()
                .map(|text| Micros::parse(text).map_or_else(|| text.to_owned(), Micros::text)),
            at_limit: word(&cap.at_limit).ok_or_else(|| bad("an at_limit"))?,
            warn_percent: cap
                .warn_percent
                .and_then(|percent| u32::try_from(percent).ok()),
            uncounted: word(&cap.uncounted).ok_or_else(|| bad("an uncounted"))?,
            note: cap.note.clone(),
            enabled: cap.enabled,
            revision: cap.revision,
            created_by: self.names.user(cap.created_by),
            created_at: Time::new(cap.created_at),
            updated_at: Time::new(cap.updated_at),
            can_edit,
        })
    }
}

/// A cap whose standing is to be shown, for one person where it is counted per person.
struct Shown {
    cap: CapRow,
    person: Option<Id>,
    replaced_by: Option<Id>,
}

/// How each of the caps stands, for the caller: with whether they may change each and let a turn past it.
async fn statuses(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    shown: Vec<Shown>,
) -> ApiResult<Vec<CapStatus>> {
    let mut computed = Vec::with_capacity(shown.len());
    let mut allowed: HashMap<(Option<Id>, Option<Id>), (bool, bool)> = HashMap::new();
    for each in shown {
        let status = engine::status(conn, &each.cap, each.person, each.replaced_by, ctx.now)
            .await
            .map_err(ApiError::internal)?;
        let key = (each.cap.node_id, each.cap.provider_id);
        let (can_edit, can_bypass) = match allowed.get(&key) {
            Some(pair) => *pair,
            None => {
                let home = Home::of(each.cap.node_id, each.cap.provider_id);
                let pair = (
                    may(conn, principal, home, Need::Manage).await?,
                    may_pass(conn, principal, home, Policy::default())
                        .await
                        .map_err(ApiError::internal)?,
                );
                allowed.insert(key, pair);
                pair
            }
        };
        computed.push((each.cap, status, can_edit, can_bypass));
    }
    let caps: Vec<&CapRow> = computed.iter().map(|each| &each.0).collect();
    let mut people: Vec<Id> = Vec::new();
    for (_, status, _, _) in &computed {
        people.extend(status.standing.person);
        people.extend(status.bypassed.last_by);
    }
    let known = Known::load(conn, principal, &caps, &people).await?;
    computed
        .iter()
        .map(|(cap, status, can_edit, can_bypass)| {
            let standing = &status.standing;
            Ok(CapStatus {
                forecast: cap_forecast(standing, ctx.now),
                cap: known.cap(cap, *can_edit)?,
                person: standing.person.map(|id| known.names.user(id)),
                used: standing.used.text(),
                in_progress: standing.in_progress.text(),
                waiting: status.waiting,
                left_out: CapLeftOut {
                    unrated_tokens: standing.unrated_tokens,
                    unmetered_threads: status.unmetered_threads,
                },
                state: match standing.state() {
                    engine::State::Ok => CapState::Ok,
                    engine::State::Warning => CapState::Warning,
                    engine::State::Reached => CapState::Reached,
                },
                period_ends_at: standing.window.ends_at.map(Time::new),
                replaced_by: status.replaced_by,
                can_bypass: *can_bypass,
                bypasses: CapBypasses {
                    count: status.bypassed.count,
                    last: match (status.bypassed.last_by, status.bypassed.last_at) {
                        (Some(by), Some(at)) => Some(CapBypassLast {
                            by: known.names.user(by),
                            at: Time::new(at),
                        }),
                        _ => None,
                    },
                },
            })
        })
        .collect()
}

/// Daily rollups give the exact period count without an extra ledger scan for each cap.
fn cap_forecast(
    standing: &engine::Standing,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<orch_provider_contract::forecast::UsageForecast> {
    use orch_provider_contract::forecast::{ForecastSource, forecast};
    let start = standing.window.start?.and_hms_opt(0, 0, 0)?.and_utc();
    let end = standing.window.ends_at?;
    let amount: f64 = standing.amount?.text().parse().ok()?;
    if amount <= 0.0 || !amount.is_finite() {
        return None;
    }
    let used: f64 = standing.counted().text().parse().ok()?;
    forecast(
        start,
        end,
        end - start,
        now,
        ForecastSource::PeriodAverage(used / amount * 100.0),
    )
}

/// The rows that show a cap: one for a cap that is shared, and for a cap counted per person one for each person
/// who has used any of it in the period, or a single one for nobody where no one has.
async fn rows_of(conn: &mut Conn, cap: &CapRow, now: DateTime<Utc>) -> ApiResult<Vec<Shown>> {
    if !cap.per_person() {
        return Ok(vec![Shown {
            cap: cap.clone(),
            person: None,
            replaced_by: None,
        }]);
    }
    let since = engine::window(cap, now).start;
    let mut people: BTreeSet<Id> = BTreeSet::new();
    let users = rows::users_of(conn, cap, since, now).await?;
    let covered = engine::used_of_covered_models(conn, cap, users)
        .await
        .map_err(ApiError::internal)?;
    for person in covered {
        if engine::covers_person(conn, cap, person, None, now)
            .await
            .map_err(ApiError::internal)?
        {
            people.insert(person);
        }
    }
    if people.is_empty() {
        return Ok(vec![Shown {
            cap: cap.clone(),
            person: None,
            replaced_by: None,
        }]);
    }
    let siblings = rows::siblings(conn, cap).await?;
    let mut out = Vec::new();
    for person in people {
        let mut covering = Vec::new();
        for sibling in &siblings {
            if sibling.id == cap.id
                || engine::covers_person(conn, sibling, person, None, now)
                    .await
                    .map_err(ApiError::internal)?
            {
                covering.push(sibling.clone());
            }
        }
        let replaced_by = engine::precedence(covering)
            .into_iter()
            .find(|applied| applied.cap.id == cap.id)
            .and_then(|applied| applied.replaced_by);
        out.push(Shown {
            cap: cap.clone(),
            person: Some(person),
            replaced_by,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- reading

/// Reads the place the query asks about: exactly one of a node, a provider and the system.
fn place_of(query: &UsageCapListQuery) -> ApiResult<(Home, DefinedOn)> {
    let system = query.system == Some(true);
    match (query.node_id, query.provider_id, system) {
        (Some(node), None, false) => Ok((Home::Node(node), DefinedOn::Node(node))),
        (None, Some(provider), false) => Ok((
            Home::Provider {
                provider,
                node: None,
            },
            DefinedOn::Provider(provider),
        )),
        (None, None, true) => Ok((Home::System, DefinedOn::System)),
        _ => Err(invalid(
            "node_id",
            "name exactly one of node_id, provider_id and system=true",
        )),
    }
}

/// The enabled caps that could cover work at a node: those defined on the nodes on the way down to it, those of
/// the system, and those of the providers that can be used there.
async fn on_the_way(conn: &mut Conn, node: Id) -> ApiResult<Vec<CapRow>> {
    let place = rollup::place_of(conn, node)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let root = rows::node_org(conn, node)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let path = threads::node_path(conn, node).await?;
    let usable: HashSet<Id> = rollup::providers_usable_at(conn, root, Some(place))
        .await?
        .into_iter()
        .collect();
    Ok(rows::on_the_way_to(conn, &path)
        .await?
        .into_iter()
        .filter(|cap| {
            cap.provider_id
                .is_none_or(|provider| usable.contains(&provider))
        })
        .collect())
}

/// Lists the caps defined in a place with how each stands, and with `covering` those defined elsewhere that
/// cover work at the node.
pub async fn list(ctx: Ctx, query: UsageCapListQuery, (): ()) -> ApiResult<Vec<CapStatus>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (home, defined) = place_of(&query)?;
    require(&mut conn, &principal, home, Need::View).await?;
    let mut caps = rows::defined_on(&mut conn, defined).await?;
    if query.covering == Some(true) {
        let DefinedOn::Node(node) = defined else {
            return Err(invalid(
                "covering",
                "needs node_id: it asks what covers work at a node",
            ));
        };
        let here: HashSet<Id> = caps.iter().map(|cap| cap.id).collect();
        caps.extend(
            on_the_way(&mut conn, node)
                .await?
                .into_iter()
                .filter(|cap| !here.contains(&cap.id)),
        );
    }
    let mut shown = Vec::new();
    for cap in &caps {
        shown.extend(rows_of(&mut conn, cap, ctx.now).await?);
    }
    statuses(&ctx, &mut conn, &principal, shown).await
}

/// The caller's applicable caps defined on a provider already admitted by its ordinary View check.
pub(crate) async fn for_provider_choices(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    providers: &[Id],
) -> ApiResult<HashMap<Id, Vec<CapStatus>>> {
    let mut covered = Vec::new();
    for cap in rows::on_providers(conn, providers).await? {
        // A cap may also name a place. Its ordinary visibility check still governs that name.
        if cap.enabled
            && (cap.node_id.is_none()
                || may(
                    conn,
                    principal,
                    Home::of(cap.node_id, cap.provider_id),
                    Need::View,
                )
                .await?)
            && engine::covers_person(conn, &cap, principal.user(), None, ctx.now)
                .await
                .map_err(ApiError::internal)?
        {
            covered.push(cap);
        }
    }
    let shown: Vec<Shown> = engine::precedence(covered)
        .into_iter()
        .filter(|each| each.replaced_by.is_none())
        .map(|each| Shown {
            person: each.cap.per_person().then_some(principal.user()),
            cap: each.cap,
            replaced_by: None,
        })
        .collect();
    let mut grouped: HashMap<Id, Vec<CapStatus>> = HashMap::new();
    if !shown.is_empty() {
        for status in statuses(ctx, conn, principal, shown).await? {
            if let Some(provider) = &status.cap.provider {
                grouped.entry(provider.id).or_default().push(status);
            }
        }
    }
    Ok(grouped)
}

/// Lists everything that bounds work at a node, with the caller's own share of the caps counted per person, or
/// somebody else's for a reader of usage.
pub async fn limits(ctx: Ctx, query: UsageLimitsGetQuery, (): ()) -> ApiResult<Limits> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let node = query.node_id;
    authorization::require(
        &mut conn,
        &principal,
        node,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    let person = match query.user_id {
        Some(other) if other != principal.user() => {
            authorization::require(
                &mut conn,
                &principal,
                node,
                exact(Permission::UsageView),
                Act::Use,
                Policy::default(),
            )
            .await?;
            other
        }
        _ => principal.user(),
    };
    let mut covering = Vec::new();
    for cap in on_the_way(&mut conn, node).await? {
        if engine::covers_person(&mut conn, &cap, person, Some(node), ctx.now)
            .await
            .map_err(ApiError::internal)?
        {
            covering.push(cap);
        }
    }
    let shown = share_of(covering, person);
    Ok(Limits {
        caps: statuses(&ctx, &mut conn, &principal, shown).await?,
    })
}

/// Current caps for exactly one prospective turn, using the same coverage and precedence as preflight.
pub(crate) async fn for_turn(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ask: &engine::Ask,
) -> ApiResult<Vec<CapStatus>> {
    let shown = engine::covering(conn, ask, ctx.now)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|each| each.replaced_by.is_none())
        .map(|each| Shown {
            person: each.cap.per_person().then_some(ask.person).flatten(),
            replaced_by: None,
            cap: each.cap,
        })
        .collect();
    statuses(ctx, conn, principal, shown).await
}

/// Reached caps with current bypass authority and request targets, for the author of a pending input.
pub(crate) async fn reached_for_thread(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    input: Option<&threads::InputRow>,
) -> ApiResult<Vec<orch_api_contract::threads::LimitReached>> {
    use crate::thread_access::AuthzAccess;
    use orch_api_contract::threads::LimitReached;
    use orch_engine::access::{Limit, ResourceAccess};
    // Refusal previews expose the five nearest holders throughout the API.
    const ASK_LIMIT: usize = 5;
    let author = input.map_or(Some(principal.user()), |input| input.author_id);
    let Some(ask) = engine::ask_for_thread(conn, thread, author)
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(Vec::new());
    };
    // Stored entries carry ids rather than public user references.
    let stored = input
        .and_then(|input| input.limit_bypass.as_ref())
        .and_then(Json::as_array);
    let mut reached = Vec::new();
    let access = AuthzAccess::new(std::sync::Arc::clone(&ctx.app.clock));
    for mut status in for_turn(ctx, conn, principal, &ask).await? {
        if status.state != CapState::Reached {
            continue;
        }
        let cap = status.cap.id;
        let limit = Limit::Cap { cap };
        status.can_bypass = access
            .may_pass(
                conn,
                principal.user(),
                principal.caller().as_admin(),
                &limit,
            )
            .await
            .map_err(ApiError::internal)?;
        let mut passed_by = None;
        for entry in stored.into_iter().flatten() {
            let includes = entry
                .get("caps")
                .and_then(Json::as_array)
                .is_some_and(|ids| {
                    ids.iter()
                        .any(|id| id.as_str().and_then(|id| id.parse::<Id>().ok()) == Some(cap))
                });
            let by = entry
                .get("by")
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok());
            let as_admin = entry
                .get("as_admin")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            if includes
                && let Some(by) = by
                && access
                    .may_pass(conn, by, as_admin, &limit)
                    .await
                    .map_err(ApiError::internal)?
            {
                passed_by = refusals::user_refs(conn, &[by]).await?.remove(&by);
                break;
            }
        }
        let home = Home::of(
            status.cap.node.as_ref().map(|node| node.id),
            status.cap.provider.as_ref().map(|provider| provider.id),
        );
        let (ask, ask_more) = if status.can_bypass {
            (Vec::new(), 0)
        } else {
            match orch_authz::caps::check_pass(conn, principal, home, Policy::default())
                .await
                .map_err(ApiError::internal)?
            {
                Verdict::Refused(refusal) => {
                    let missing = refusals::missing_access(conn, &refusal.missing).await?;
                    missing
                        .into_iter()
                        .next()
                        .map_or((Vec::new(), 0), |first| (first.ask, first.ask_more))
                }
                Verdict::SystemAdminOnly => {
                    let ids = orch_authz::caps::managers(conn, home, Time::new(ctx.now))
                        .await
                        .map_err(ApiError::internal)?;
                    let more =
                        i64::try_from(ids.len().saturating_sub(ASK_LIMIT)).unwrap_or(i64::MAX);
                    let mut names = refusals::user_refs(conn, &ids).await?;
                    (
                        ids.into_iter()
                            .take(ASK_LIMIT)
                            .filter_map(|id| names.remove(&id))
                            .collect(),
                        more,
                    )
                }
                Verdict::Allowed => (Vec::new(), 0),
            }
        };
        reached.push(LimitReached {
            can_bypass: status.can_bypass,
            cap: Some(status),
            provider_limit: None,
            passed_by,
            ask,
            ask_more,
        });
    }
    Ok(reached)
}

/// The caps of one person as rows to show: a shared cap as it stands, one counted per person as the person's
/// share, with the cap that applies in its stead where there is one. In the order the caps were made.
fn share_of(covering: Vec<CapRow>, person: Id) -> Vec<Shown> {
    let mut applied = engine::precedence(covering);
    applied.sort_by_key(|each| (each.cap.created_at, each.cap.id));
    applied
        .into_iter()
        .map(|each| Shown {
            person: each.cap.per_person().then_some(person),
            replaced_by: each.replaced_by,
            cap: each.cap,
        })
        .collect()
}

/// Lists the caller's own share of every cap counted per person that covers them, wherever it is defined.
pub async fn mine(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<CapStatus>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let caller = principal.user();
    let mut covering = Vec::new();
    for cap in rows::per_person(&mut conn).await? {
        if engine::covers_person(&mut conn, &cap, caller, None, ctx.now)
            .await
            .map_err(ApiError::internal)?
        {
            covering.push(cap);
        }
    }
    let shown = share_of(covering, caller);
    statuses(&ctx, &mut conn, &principal, shown).await
}

/// Lists what waits for room in a cap, in the order it will run. Somebody who reads the cap sees every row named
/// as far as they may see the thread; somebody the cap only covers sees their own rows named and the others as
/// places in the queue.
pub async fn waiting(ctx: Ctx, (): (), (): ()) -> ApiResult<UsageCapWaitingListReply> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let cap = rows::get(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let caller = principal.user();
    let reader = may(&mut conn, &principal, engine::home_of(&cap), Need::View).await?;
    if !reader {
        let covered = engine::covers_person(&mut conn, &cap, caller, None, ctx.now)
            .await
            .map_err(ApiError::internal)?;
        if !covered {
            // The refusal says what they lack.
            require(&mut conn, &principal, engine::home_of(&cap), Need::View).await?;
        }
    }
    let queue = engine::waiting_for(&mut conn, &cap, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let threads: Vec<Id> = queue.iter().map(|waiter| waiter.thread_id).collect();
    let people: Vec<Id> = queue.iter().filter_map(|waiter| waiter.author_id).collect();
    let names = Names::load(&mut conn, &principal, &people, &[], &threads).await?;
    let items = queue
        .iter()
        .map(|waiter| {
            let mine = waiter.author_id == Some(caller);
            let named = reader || mine;
            let thread = names.node(waiter.thread_id, NodeKind::Thread);
            UsageCapWaiting {
                thread: (named && !thread.redacted).then_some(thread),
                author: waiter
                    .author_id
                    .filter(|_| named)
                    .map(|author| names.user(author)),
                since: Time::new(waiter.since),
                mine,
            }
        })
        .collect();
    Ok(UsageCapWaitingListReply { items })
}

// ---------------------------------------------------------------- writing

/// The words of one cap, checked.
struct Checked {
    subject_kind: &'static str,
    subject_id: Option<Id>,
    subject_role: Option<String>,
    split: &'static str,
    models: Vec<String>,
    tiers: Vec<String>,
    harness_kinds: Vec<String>,
    meter: &'static str,
    period: Option<&'static str>,
    counted_from: Option<NaiveDate>,
    amount: Option<String>,
    at_limit: &'static str,
    warn_percent: Option<i16>,
    uncounted: &'static str,
    note: String,
}

/// The models or harness kinds a request names: each without space and not empty, at most [`WORDS_MAX`], in a
/// form that makes two caps that name the same ones the same cap. An empty one is refused, since a list that is
/// empty covers every model.
fn named(path: &str, listed: &[String], lower: bool) -> ApiResult<Vec<String>> {
    if listed.len() > WORDS_MAX {
        return Err(invalid(path, &format!("names at most {WORDS_MAX}")));
    }
    let mut words = Vec::with_capacity(listed.len());
    for each in listed {
        let each = each.trim();
        if each.is_empty() || each.chars().count() > WORD_MAX_CHARS {
            return Err(invalid(
                path,
                &format!("holds a name that is empty or longer than {WORD_MAX_CHARS} characters"),
            ));
        }
        if each.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(invalid(path, "holds a name with a space in it"));
        }
        words.push(if lower {
            each.to_lowercase()
        } else {
            each.to_owned()
        });
    }
    Ok(rows::normalised(&words))
}

/// The models a request names, as [`named`], where each `@<group>` names a group that can exist: a group is
/// named by what a provider's managers may call one.
fn named_models(path: &str, listed: &[String]) -> ApiResult<Vec<String>> {
    let models = named(path, listed, true)?;
    for (index, pattern) in models.iter().enumerate() {
        if let Some(group) = ModelPattern::group_of(pattern)
            && !is_model_group_name(group)
        {
            return Err(invalid(
                &format!("{path}[{index}]"),
                "a group is written @ and its name: 1 to 40 of a-z, 0-9, - and _, beginning with a letter or a digit",
            ));
        }
    }
    Ok(models)
}

/// An amount in the unit of its meter, as the canonical text of the number.
fn amount_of(path: &str, meter_name: &str, text: &str) -> ApiResult<String> {
    let parsed = Micros::parse_exact(text).ok_or_else(|| {
        invalid(
            path,
            &format!("is a decimal number of at most {CAP_CREDIT_DECIMALS} places, as text"),
        )
    })?;
    if meter_name != meter::CREDITS && parsed.to_whole().is_none() {
        return Err(invalid(
            path,
            &format!("is a whole number of {}", counts(meter_name)),
        ));
    }
    Ok(parsed.text())
}

fn note_of(text: &str) -> ApiResult<String> {
    if text.chars().count() > NOTE_MAX_CHARS {
        return Err(invalid(
            "note",
            &format!("is at most {NOTE_MAX_CHARS} characters"),
        ));
    }
    Ok(text.to_owned())
}

fn warn_of(percent: u32) -> ApiResult<i16> {
    if !(CAP_WARN_PERCENT_MIN..=CAP_WARN_PERCENT_MAX).contains(&percent) {
        return Err(invalid(
            "warn_percent",
            &format!("is from {CAP_WARN_PERCENT_MIN} to {CAP_WARN_PERCENT_MAX}"),
        ));
    }
    i16::try_from(percent).map_err(|_| invalid("warn_percent", "is not a percent"))
}

fn day_of(text: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| invalid("counted_from", "is a day, written YYYY-MM-DD"))
}

/// What a request to make a cap says, checked against itself.
fn check(input: &CapInput) -> ApiResult<Checked> {
    let (subject_kind, subject_id, subject_role) = match &input.subject {
        CapSubjectInput::Everyone => (subject::EVERYONE, None, None),
        CapSubjectInput::User { id } => (subject::USER, Some(*id), None),
        CapSubjectInput::Group { id } => (subject::GROUP, Some(*id), None),
        CapSubjectInput::Members { id } => (subject::MEMBERS, Some(*id), None),
        CapSubjectInput::Role { role, at } => (
            subject::ROLE,
            *at,
            Some(
                serde_json::to_value(role)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or_else(|| invalid("subject.role", "is not a role"))?,
            ),
        ),
    };
    if subject_kind == subject::USER && input.split != CapSplit::Shared {
        return Err(invalid(
            "split",
            "is `shared` for a cap that names one person: the amount is theirs",
        ));
    }
    if subject_kind == subject::ROLE
        && (input.split != CapSplit::Each || (input.provider_id.is_some() && subject_id.is_none()))
    {
        return Err(invalid(
            "subject",
            "a cap for a role is counted for each person, and is not defined on a provider",
        ));
    }
    let meter_name = meter_word(input.meter);
    let running = input.meter == CapMeter::Running;
    if running {
        for (path, given) in [
            ("period", input.period.is_some()),
            ("warn_percent", input.warn_percent.is_some()),
            ("at_limit", input.at_limit.is_some()),
            ("uncounted", input.uncounted.is_some()),
            ("counted_from", input.counted_from.is_some()),
        ] {
            if given {
                return Err(invalid(
                    path,
                    "a cap on turns running at once has no period, warning, at_limit or uncounted",
                ));
            }
        }
    } else if input.period.is_none() {
        return Err(invalid("period", "is required unless meter is `running`"));
    }
    let counted_from = input.counted_from.as_deref().map(day_of).transpose()?;
    if counted_from.is_some() && input.period != Some(CapPeriod::Total) {
        return Err(invalid("counted_from", "is for the period `total`"));
    }
    Ok(Checked {
        subject_kind,
        subject_id,
        subject_role,
        split: split_word(input.split),
        models: named_models("models", input.models.as_deref().unwrap_or_default())?,
        tiers: named("tiers", input.tiers.as_deref().unwrap_or_default(), false)?,
        harness_kinds: named(
            "harness_kinds",
            input.harness_kinds.as_deref().unwrap_or_default(),
            false,
        )?,
        meter: meter_name,
        period: input.period.map(period_word),
        counted_from,
        amount: input
            .amount
            .as_deref()
            .map(|text| amount_of("amount", meter_name, text))
            .transpose()?,
        at_limit: at_limit_word(input.at_limit.unwrap_or(CapAtLimit::Stop)),
        warn_percent: input.warn_percent.map(warn_of).transpose()?,
        uncounted: uncounted_word(input.uncounted.unwrap_or(CapUncounted::Allow)),
        note: note_of(input.note.as_deref().unwrap_or_default())?,
    })
}

/// The root org whose tree a group of the cap must belong to: the node's, else the provider's home's.
async fn tree_of(conn: &mut Conn, node: Option<Id>, provider: Option<Id>) -> ApiResult<Option<Id>> {
    Ok(match (node, provider) {
        (Some(node), _) => rows::node_org(conn, node).await?,
        (None, Some(provider)) => rows::provider_org(conn, provider).await?,
        (None, None) => None,
    })
}

/// Writes the audit row of an act on a cap.
async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    cap: &CapRow,
    what: &str,
    details: Json,
) -> ApiResult<()> {
    let who = actor_name(ctx)?;
    let summary = format!("{who} {what} a cap on {}", counts(&cap.meter));
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        action.object_kind().unwrap_or("usage_cap"),
        Some(cap.id),
        &summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = cap.node_id;
    entry.org_id = tree_of(conn, cap.node_id, cap.provider_id).await?;
    entry.details = details;
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

/// What the audit trail keeps of a cap.
fn about(cap: &CapRow) -> Json {
    json!({
        "cap": cap.id,
        "node": cap.node_id,
        "provider": cap.provider_id,
        "subject": cap.subject_kind,
        "subject_id": cap.subject_id,
        "subject_role": cap.subject_role,
        "split": cap.split,
        "models": cap.models,
        "tiers": cap.tiers,
        "harness_kinds": cap.harness_kinds,
        "meter": cap.meter,
        "period": cap.period,
        "amount": cap.amount,
        "at_limit": cap.at_limit,
        "enabled": cap.enabled,
    })
}

/// Tells the engine that a cap was made, changed or removed, once the change is kept: it looks at the turns
/// the cap covers, and at what waits. A failure to look is for the next report of usage to put right.
async fn engine_told(ctx: &Ctx, cap: Id) {
    if let Err(error) = ctx.app.engine.cap_changed(cap).await {
        tracing::warn!(%error, %cap, "the turns a cap covers could not be looked at; the next report of usage looks again");
    }
}

/// Makes a cap.
pub async fn create(mut ctx: Ctx, (): (), body: CapInput) -> ApiResult<UsageCap> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let home = Home::of(body.node_id, body.provider_id);
    require(&mut tx, &principal, home, Need::Manage).await?;
    let checked = check(&body)?;
    match (checked.subject_kind, checked.subject_id) {
        (subject::USER, Some(user)) => {
            if !refusals::user_refs(&mut tx, &[user])
                .await?
                .contains_key(&user)
            {
                return Err(invalid("subject.id", "names nobody"));
            }
        }
        (subject::GROUP, Some(group)) => {
            let found = access::groups_full(&mut tx, &[group]).await?;
            let found = found
                .first()
                .ok_or_else(|| invalid("subject.id", "names no group"))?;
            authorization::require(
                &mut tx,
                &principal,
                found.org_id,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
        }
        (subject::ROLE | subject::MEMBERS, Some(source)) => {
            let (_, kind) = crate::routes::nodes::placed(&mut tx, source).await?;
            if kind == orch_api_contract::nodes::NodeKind::Thread
                || (checked.subject_kind == subject::MEMBERS
                    && kind != orch_api_contract::nodes::NodeKind::Org)
            {
                return Err(invalid("subject.at", "is no place of that kind"));
            }
            authorization::require(
                &mut tx,
                &principal,
                source,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
        }
        _ => {}
    }
    let made = rows::insert(
        &mut tx,
        &NewCap {
            id: new_id(),
            node_id: body.node_id,
            provider_id: body.provider_id,
            subject_kind: checked.subject_kind,
            subject_id: checked.subject_id,
            subject_role: checked.subject_role.as_deref(),
            split: checked.split,
            models: &checked.models,
            tiers: &checked.tiers,
            harness_kinds: &checked.harness_kinds,
            meter: checked.meter,
            period: checked.period,
            counted_from: checked.counted_from,
            amount: checked.amount.as_deref(),
            at_limit: checked.at_limit,
            warn_percent: checked.warn_percent,
            uncounted: checked.uncounted,
            note: &checked.note,
            created_by: principal.user(),
            at: ctx.now,
        },
    )
    .await?;
    let Some(cap) = made else {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "a cap with the same node, provider, subject, split, models, tiers, harness kinds, meter and period exists",
        ));
    };
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::UsageCapCreated),
        &cap,
        "added",
        about(&cap),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    engine_told(&ctx, cap.id).await;
    let mut conn = ctx.app.db.acquire().await?;
    let known = Known::load(&mut conn, &principal, &[&cap], &[]).await?;
    known.cap(&cap, true)
}

/// What a change to a cap says, checked against the cap.
struct Changing {
    models: Option<Vec<String>>,
    tiers: Option<Vec<String>>,
    harness_kinds: Option<Vec<String>>,
    counted_from: Option<Option<NaiveDate>>,
    amount: Option<Option<String>>,
    at_limit: Option<&'static str>,
    warn_percent: Option<Option<i16>>,
    uncounted: Option<&'static str>,
    note: Option<String>,
    enabled: Option<bool>,
}

fn check_change(cap: &CapRow, body: &UsageCapUpdateBody) -> ApiResult<Changing> {
    let running = cap.meter == meter::RUNNING;
    if running {
        for (path, given) in [
            ("at_limit", body.at_limit.is_some()),
            ("uncounted", body.uncounted.is_some()),
            ("warn_percent", matches!(body.warn_percent, Some(Some(_)))),
            ("counted_from", matches!(body.counted_from, Some(Some(_)))),
        ] {
            if given {
                return Err(invalid(
                    path,
                    "a cap on turns running at once has no period, warning, at_limit or uncounted",
                ));
            }
        }
    }
    let counted_from = body
        .counted_from
        .as_ref()
        .map(|day| day.as_deref().map(day_of).transpose())
        .transpose()?;
    if matches!(counted_from, Some(Some(_))) && cap.period.as_deref() != Some(period::TOTAL) {
        return Err(invalid("counted_from", "is for the period `total`"));
    }
    Ok(Changing {
        models: body
            .models
            .as_deref()
            .map(|listed| named_models("models", listed))
            .transpose()?,
        tiers: body
            .tiers
            .as_deref()
            .map(|listed| named("tiers", listed, false))
            .transpose()?,
        harness_kinds: body
            .harness_kinds
            .as_deref()
            .map(|listed| named("harness_kinds", listed, false))
            .transpose()?,
        counted_from,
        amount: body
            .amount
            .as_ref()
            .map(|amount| {
                amount
                    .as_deref()
                    .map(|text| amount_of("amount", &cap.meter, text))
                    .transpose()
            })
            .transpose()?,
        at_limit: body.at_limit.map(at_limit_word),
        warn_percent: body
            .warn_percent
            .map(|percent| percent.map(warn_of).transpose())
            .transpose()?,
        uncounted: body.uncounted.map(uncounted_word),
        note: body.note.as_deref().map(note_of).transpose()?,
        enabled: body.enabled,
    })
}

/// Which members a change touches, by name, for the audit trail.
fn touched(body: &UsageCapUpdateBody) -> Vec<&'static str> {
    [
        ("models", body.models.is_some()),
        ("tiers", body.tiers.is_some()),
        ("harness_kinds", body.harness_kinds.is_some()),
        ("counted_from", body.counted_from.is_some()),
        ("amount", body.amount.is_some()),
        ("at_limit", body.at_limit.is_some()),
        ("warn_percent", body.warn_percent.is_some()),
        ("uncounted", body.uncounted.is_some()),
        ("note", body.note.is_some()),
        ("enabled", body.enabled.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, given)| given.then_some(name))
    .collect()
}

/// Changes a cap against the revision the caller saw.
pub async fn update(mut ctx: Ctx, (): (), body: UsageCapUpdateBody) -> ApiResult<UsageCap> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let cap = rows::get(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require(&mut tx, &principal, engine::home_of(&cap), Need::Manage).await?;
    let touching = touched(&body);
    if touching.is_empty() {
        return Err(invalid("", "names nothing to change"));
    }
    let changing = check_change(&cap, &body)?;
    let outcome = rows::update(
        &mut tx,
        id,
        body.expected_revision,
        &Change {
            models: changing.models.as_deref(),
            tiers: changing.tiers.as_deref(),
            harness_kinds: changing.harness_kinds.as_deref(),
            counted_from: changing.counted_from,
            amount: changing.amount.as_ref().map(Option::as_deref),
            at_limit: changing.at_limit,
            warn_percent: changing.warn_percent,
            uncounted: changing.uncounted,
            note: changing.note.as_deref(),
            enabled: changing.enabled,
        },
        ctx.now,
    )
    .await?;
    let changed = match outcome {
        Updated::Done(changed) => *changed,
        Updated::Stale(current) => {
            tx.rollback().await.map_err(orch_db::DbError::from)?;
            let mut conn = ctx.app.db.acquire().await?;
            let known = Known::load(&mut conn, &principal, &[&current], &[]).await?;
            let shown = known.cap(&current, true)?;
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the cap was changed since it was read",
            )
            .with("current", shown));
        }
        Updated::Exists => {
            return Err(ApiError::new(
                ErrorCode::ConflictExists,
                "with these models, tiers and harness kinds it would be a cap that exists",
            ));
        }
        Updated::Gone => return Err(ApiError::not_found()),
    };
    let mut details = about(&changed);
    details["changed"] = json!(touching);
    details["revision"] = json!(changed.revision);
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::UsageCapUpdated),
        &changed,
        "changed",
        details,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    engine_told(&ctx, changed.id).await;
    let mut conn = ctx.app.db.acquire().await?;
    let known = Known::load(&mut conn, &principal, &[&changed], &[]).await?;
    known.cap(&changed, true)
}

/// Removes a cap.
pub async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let cap = rows::get(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require(&mut tx, &principal, engine::home_of(&cap), Need::Manage).await?;
    if !rows::delete(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::UsageCapDeleted),
        &cap,
        "removed",
        about(&cap),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    engine_told(&ctx, id).await;
    Ok(())
}
