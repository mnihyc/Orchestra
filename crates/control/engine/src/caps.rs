//! Usage caps: coverage, per-person precedence, room in tokens, in credits and in turns running at once, held inputs and waiting threads released in order, stopping turns and notices; and the limit bypass, by which one turn is let past caps that are reached, an entry for each person who did, checked again at its start and recorded.
//!
//! **What a cap covers.** A cap covers a turn of a thread, on a provider with a
//! model, by an agent of a harness kind, put down to a person (the author of the
//! input that starts it, and nobody for an input no person sent), when the node it
//! names lies on the thread's way down from the root org, the provider is its
//! provider, one of its patterns names the model (the one rule of
//! [`ModelPattern`], which also names a group of the provider's models as `@<group>`),
//! the service tier the turn asks for is among its tiers (a cap that names none covers the ordinary
//! tier too), the harness kind is among its kinds, and the person is its subject: everybody, that person, a group they are in, or the role they hold at
//! the thread (that role, and not a higher one). A cap counted per person covers
//! only use that is put down to a person.
//!
//! **Which cap a person is held to.** All the caps that cover a turn apply
//! together, but for caps counted per person that agree in node, provider, models,
//! tiers, harness kinds, meter and period: of those exactly one applies to a person,
//! the cap that names them, else the largest of the group and role caps that match
//! them (no limit is the largest), else the cap for everyone ([`precedence`]).
//!
//! **Room.** A cap in tokens or credits has room while what was used in its
//! period (the daily rollups), together with what turns still running reported
//! beyond what the ledger counted of them already, is below its amount. A cap on
//! turns running at once has room while the turns it covers that are under way
//! are fewer than its amount, counted under a lock on each such cap so that two
//! pre-flights never both see the last free place. The period is in UTC: a day, a
//! week from Monday, a calendar month, or everything since `counted_from`.
//!
//! **Before a turn starts** (`Engine::check_caps`) every covering cap that has a
//! limit is looked at. A cap without room that is shared blocks the thread
//! (`quota_exceeded`); one that is counted per person holds the input of that
//! person, which keeps its place and is passed over, so that one person's
//! allowance never stops their teammates; a cap that says `block` for what it
//! cannot measure (nobody reports the use, or the model has no rate) blocks too.
//! When all have room, a lease on the forwarded path is opened with what is left
//! of the tightest cap that would stop the turn ([`crate::leases::allowance_for`]).
//!
//! **While a turn runs** its caps are looked at again whenever its usage is
//! reported, no more often than `[engine] cap_check_secs` for a thread. A cap that
//! is reached and says `stop` ends every running turn it covers, as an interrupt
//! of a person would, with the code `cap_reached`, and closes their leases;
//! `finish_turn` does nothing yet. A cap a turn was let past does not stop it.
//!
//! **The limit bypass.** An input may carry an entry for each person who let it
//! past: the caps, and whether the provider's own limit. An entry counts only
//! while whoever set it holds, now, the right to pass that cap where it is defined
//! (`orch_authz::caps`); an entry that no longer holds, or names a cap that has
//! room, is dropped. The turn that starts past a reached cap writes, in the same
//! transaction, a row for it, a line of the audit trail and a marker in the thread.
//!
//! **Notices.** The first time a cap's use passes its warning percent in a period,
//! and the first time it is reached, the people it concerns are told, once for
//! each cap, person and period; reaching it is audited. A cap on running turns says
//! nothing: being full is its ordinary state.

use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::ingest;
use crate::providers::tokens_of;
use crate::rates::RateBook;
use crate::turns::{InputAfter, TurnEnd, error_json};
use crate::unit::Unit;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::roles::Role;
use orch_agent_contract::arrangement::Metering;
use orch_agent_contract::discovery::ModelPattern;
use orch_agent_contract::parameters::{ModelParameters, ParameterDef, Rate};
use orch_authz::caps::Home;
use orch_authz::{Caller, Policy, Principal};
use orch_db::audit::{self, Actor, Origin};
use orch_db::queries::notifications::outbox::Snapshot;
use orch_db::queries::notifications::{people, routes};
use orch_db::queries::resources::thread_engine as resources;
use orch_db::queries::threads::{self, InputRow, TurnRow, turn_state};
use orch_db::usage::caps::{
    self as rows, BypassTally, CapRow, NewBypass, TierAsked, at_limit, level, meter, period,
    subject, uncounted,
};
use orch_db::usage::micros::Micros;
use orch_db::{Conn, queries::hierarchy};
use orch_link::streams::DeliveredEvent;
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The `error.code` of a turn that a cap ended, and the `close_reason` of its lease.
pub const CAP_REACHED: &str = "cap_reached";
/// How many days a week has.
const DAYS_IN_A_WEEK: i64 = 7;
/// How far back the turns let past a cap on running turns are counted, since it has no period: a day.
const RUNNING_LOOK_BACK: Duration = Duration::days(1);
/// The kind of notification that says a cap's use passed its warning percent.
const WARNING_KIND: &str = "cap.warning";
/// The kind of notification that says a cap was reached.
const REACHED_KIND: &str = "cap.reached";

// ---------------------------------------------------------------- the rules

/// The period a cap counts in at a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The first day of the period, UTC; none for everything from the beginning.
    pub start: Option<NaiveDate>,
    /// When the period ends and a new one begins; none for `total` and for `running`.
    pub ends_at: Option<DateTime<Utc>>,
}

fn midnight(day: NaiveDate) -> Option<DateTime<Utc>> {
    day.and_hms_opt(0, 0, 0).map(|moment| moment.and_utc())
}

/// The period a cap counts in at `now`.
#[must_use]
pub fn window(cap: &CapRow, now: DateTime<Utc>) -> Window {
    let today = now.date_naive();
    let (start, next) = match cap.period.as_deref() {
        Some(period::DAY) => (Some(today), today.succ_opt()),
        Some(period::WEEK) => {
            let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
            (Some(monday), Some(monday + Duration::days(DAYS_IN_A_WEEK)))
        }
        Some(period::MONTH) => {
            let first = today.with_day(1).unwrap_or(today);
            let following = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
            };
            (Some(first), following)
        }
        Some(period::TOTAL) => (cap.counted_from, None),
        _ => (None, None),
    };
    Window {
        start,
        ends_at: next.and_then(midnight),
    }
}

/// The day a period is told by in the record of notices: its first day, and for `total` the day counting
/// began.
fn period_key(cap: &CapRow, window: &Window) -> NaiveDate {
    window
        .start
        .or(cap.counted_from)
        .unwrap_or_else(|| cap.created_at.date_naive())
}

/// Whether one of the patterns of a cap names the model (every model where it names none), which the
/// managers of its provider put in `groups`: by its name, by a family, or by `@<group>`.
#[must_use]
pub fn covers_model(cap: &CapRow, model: &str, groups: &[String]) -> bool {
    cap.models.is_empty() || crate::models::named_by(&cap.models, model, groups)
}

/// Whether a cap covers a turn at the service tier `tier` (none is the ordinary one): a cap that names no
/// tier covers every tier, the ordinary one too, and one that names tiers covers those alone.
#[must_use]
pub fn covers_tier(cap: &CapRow, tier: Option<&str>) -> bool {
    cap.tiers.is_empty() || tier.is_some_and(|tier| cap.tiers.iter().any(|named| named == tier))
}

/// Whether a cap names a group of models, which has to be looked up in the entries of the models it covers.
fn names_a_group(cap: &CapRow) -> bool {
    cap.models
        .iter()
        .any(|pattern| ModelPattern::group_of(pattern).is_some())
}

/// The groups of the entries of the models `pairs` (a provider, a routing name), asked only for a cap that
/// names a group.
type Groups = HashMap<(Id, String), Vec<String>>;

/// Looks the groups of `pairs` up, where `cap` names a group; otherwise nothing is asked.
async fn groups_for(
    conn: &mut Conn,
    cap: &CapRow,
    pairs: Vec<(Id, String)>,
) -> Result<Groups, EngineError> {
    if !names_a_group(cap) {
        return Ok(Groups::new());
    }
    Ok(orch_db::queries::resources::groups_of(conn, &pairs).await?)
}

/// The groups looked up for one provider and model.
fn groups_in<'g>(groups: &'g Groups, provider: Id, model: &str) -> &'g [String] {
    groups
        .get(&(provider, model.to_owned()))
        .map_or(&[], Vec::as_slice)
}

/// Of the people who used something a cap covers, each with the provider and the model of what they used
/// (as [`orch_db::usage::caps::users_of`] gives them), those whose use is of a model the cap names: by its
/// name, by a family, or by a group its provider's managers put it in.
pub async fn used_of_covered_models(
    conn: &mut Conn,
    cap: &CapRow,
    used: Vec<(Id, Id, String)>,
) -> Result<std::collections::BTreeSet<Id>, EngineError> {
    let groups = groups_for(
        conn,
        cap,
        used.iter()
            .map(|(_, provider, model)| (*provider, model.clone()))
            .collect(),
    )
    .await?;
    Ok(used
        .into_iter()
        .filter(|(_, provider, model)| {
            covers_model(cap, model, groups_in(&groups, *provider, model))
        })
        .map(|(person, _, _)| person)
        .collect())
}

/// A cap that covers a turn, and the cap that applies to the person in its stead where another does.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The cap.
    pub cap: CapRow,
    /// For a cap counted per person: the cap that applies to this person instead.
    pub replaced_by: Option<Id>,
}

type IdentityKey = (
    Option<Id>,
    Option<Id>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    String,
    Option<String>,
);

fn identity(cap: &CapRow) -> IdentityKey {
    (
        cap.node_id,
        cap.provider_id,
        cap.models.clone(),
        cap.tiers.clone(),
        cap.harness_kinds.clone(),
        cap.meter.clone(),
        cap.period.clone(),
    )
}

/// How large a limit is, for choosing the largest of several: no limit is the largest.
fn size(cap: &CapRow) -> Option<Micros> {
    cap.amount.as_deref().and_then(Micros::parse)
}

fn larger(a: &CapRow, b: &CapRow) -> std::cmp::Ordering {
    match (size(a), size(b)) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

/// Which of the caps that cover a person apply to them. Caps shared by a group apply together. Among the
/// caps counted per person that have the same node, provider, models, tiers, harness kinds, meter and period,
/// exactly one applies: the one that names the person, else the largest of the group and role caps
/// (a cap with no limit being the largest, and of equals the older), else the one for everyone.
#[must_use]
pub fn precedence(covering: Vec<CapRow>) -> Vec<Applied> {
    let mut out: Vec<Applied> = Vec::new();
    let mut groups: BTreeMap<IdentityKey, Vec<CapRow>> = BTreeMap::new();
    for cap in covering {
        if cap.per_person() {
            groups.entry(identity(&cap)).or_default().push(cap);
        } else {
            out.push(Applied {
                cap,
                replaced_by: None,
            });
        }
    }
    for (_, members) in groups {
        let chosen = members
            .iter()
            .find(|cap| cap.subject_kind == subject::USER)
            .or_else(|| {
                members
                    .iter()
                    .filter(|cap| {
                        matches!(
                            cap.subject_kind.as_str(),
                            subject::GROUP | subject::ROLE | subject::MEMBERS
                        )
                    })
                    .max_by(|a, b| {
                        larger(a, b)
                            .then_with(|| b.created_at.cmp(&a.created_at))
                            .then_with(|| b.id.cmp(&a.id))
                    })
            })
            .or_else(|| {
                members
                    .iter()
                    .find(|cap| cap.subject_kind == subject::EVERYONE)
            })
            .map(|cap| cap.id);
        for cap in members {
            let replaced_by = chosen.filter(|chosen| *chosen != cap.id);
            out.push(Applied { cap, replaced_by });
        }
    }
    out.sort_by_key(|applied| applied.cap.id);
    out
}

fn is_running(cap: &CapRow) -> bool {
    cap.meter == meter::RUNNING
}

/// The person a cap is counted for: only a cap counted per person has one.
fn person_for(cap: &CapRow, person: Option<Id>) -> Option<Id> {
    if cap.per_person() { person } else { None }
}

/// What a bypass entry of an input says: who let it past what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Who let it past.
    pub by: Id,
    /// When, as the stored text.
    pub at: Json,
    /// The caps.
    pub caps: Vec<Id>,
    /// Whether the provider's own limit is passed as well.
    pub provider_limit: bool,
    /// Whether whoever let it past acted with As admin on, as the session they did it from: a
    /// system admin passes a cap of the system only so, and is asked again as they acted.
    pub as_admin: bool,
}

impl Entry {
    fn json(&self) -> Json {
        let mut entry = json!({ "by": self.by, "at": self.at, "caps": self.caps, "provider_limit": self.provider_limit });
        // Kept only where it is so, as the request wrote it.
        if self.as_admin {
            entry["as_admin"] = json!(true);
        }
        entry
    }
}

/// The entries of a stored bypass.
#[must_use]
pub fn entries_of(stored: Option<&Json>) -> Vec<Entry> {
    stored
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(Entry {
                by: entry.get("by")?.as_str()?.parse().ok()?,
                at: entry.get("at").cloned().unwrap_or(Json::Null),
                caps: entry
                    .get("caps")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str()?.parse().ok())
                    .collect(),
                provider_limit: entry
                    .get("provider_limit")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                as_admin: entry
                    .get("as_admin")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// The stored form of entries; none where nothing stands.
#[must_use]
pub fn entries_json(entries: &[Entry]) -> Option<Json> {
    let kept: Vec<Json> = entries
        .iter()
        .filter(|entry| !entry.caps.is_empty() || entry.provider_limit)
        .map(Entry::json)
        .collect();
    (!kept.is_empty()).then_some(Json::Array(kept))
}

/// Whether a stored bypass names the cap.
#[must_use]
pub fn bypass_names(stored: Option<&Json>, cap: Id) -> bool {
    entries_of(stored)
        .iter()
        .any(|entry| entry.caps.contains(&cap))
}

// ---------------------------------------------------------------- how a cap stands

/// What a turn is asked of the caps by.
#[derive(Debug, Clone)]
pub struct Ask {
    /// The thread.
    pub thread: Id,
    /// The nodes from the root org down to the thread.
    pub path: Vec<Id>,
    /// The provider it runs on.
    pub provider: Id,
    /// The model, by its routing name.
    pub model: String,
    /// The service tier it asks for; none is the ordinary one.
    pub tier: Option<String>,
    /// The harness kind of its agent.
    pub harness_kind: Option<String>,
    /// The person it is put down to.
    pub person: Option<Id>,
    /// Who reports what it uses, where that is known.
    pub metering: Option<Metering>,
}

/// How far a cap is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// There is room.
    Ok,
    /// The warning percent was passed.
    Warning,
    /// There is no room.
    Reached,
}

impl Engine {
    /// Counts each enabled cap once at its worst current share. Reuses admission's exact
    /// coverage, periods and in-progress use; user and cap identifiers never become labels.
    pub async fn measure_caps(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let mut counts = BTreeMap::<(String, &'static str), u64>::new();
        for cap in rows::enabled_definitions(&mut conn).await? {
            let mut state = State::Ok;
            if cap.split == "each" {
                let mut people = HashSet::new();
                for (person, _, _) in
                    rows::users_of(&mut conn, &cap, window(&cap, now).start, now).await?
                {
                    people.insert(person);
                }
                for running in rows::running(&mut conn, &cap, None, now).await? {
                    people.extend(running.user_id);
                }
                if cap.subject_kind == subject::USER {
                    people.extend(cap.subject_id);
                }
                for person in people {
                    state = worse(
                        state,
                        standing(&mut conn, &cap, Some(person), now).await?.state(),
                    );
                }
            } else {
                state = standing(&mut conn, &cap, None, now).await?.state();
            }
            let state = match state {
                State::Ok => "ok",
                State::Warning => "warning",
                State::Reached => "reached",
            };
            *counts.entry((cap.meter, state)).or_default() += 1;
        }
        // Replace every bounded series only after a complete successful look; a deleted cap disappears.
        for meter in [meter::TOKENS, meter::CREDITS, meter::RUNNING] {
            for state in ["ok", "warning", "reached"] {
                metrics::gauge!("orch_usage_caps","meter"=>meter,"state"=>state)
                    .set(counts.get(&(meter.to_owned(), state)).copied().unwrap_or(0) as f64);
            }
        }
        Ok(())
    }
}

fn worse(left: State, right: State) -> State {
    match (left, right) {
        (State::Reached, _) | (_, State::Reached) => State::Reached,
        (State::Warning, _) | (_, State::Warning) => State::Warning,
        _ => State::Ok,
    }
}

/// A cap, and how much of it is used by whom it is counted for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The cap.
    pub cap: CapRow,
    /// Whose share this is, for a cap counted per person.
    pub person: Option<Id>,
    /// The limit; none for no limit.
    pub amount: Option<Micros>,
    /// What is counted in the period; for a cap on running turns, how many run now.
    pub used: Micros,
    /// What running turns reported beyond what the ledger counted of them.
    pub in_progress: Micros,
    /// The period.
    pub window: Window,
    /// For a cap in credits: the tokens of models without a rate in the period.
    pub unrated_tokens: i64,
}

impl Standing {
    /// What counts against the limit: what was used and what is under way.
    #[must_use]
    pub fn counted(&self) -> Micros {
        self.used + self.in_progress
    }

    /// Whether a turn may start under it. For turns at once, starting one would make one more.
    #[must_use]
    pub fn has_room(&self) -> bool {
        self.amount.is_none_or(|amount| self.counted() < amount)
    }

    /// What is left of the limit; none where there is none.
    #[must_use]
    pub fn room_left(&self) -> Option<Micros> {
        self.amount.map(|amount| amount.floor_sub(self.counted()))
    }

    /// How far it is used.
    #[must_use]
    pub fn state(&self) -> State {
        let Some(amount) = self.amount else {
            return State::Ok;
        };
        if !self.has_room() {
            return State::Reached;
        }
        let warns = self
            .cap
            .warn_percent
            .and_then(|percent| u32::try_from(percent).ok())
            .is_some_and(|percent| self.counted().at_least_percent_of(percent, amount));
        if warns { State::Warning } else { State::Ok }
    }
}

/// The rate a model is counted at on a provider by the provider's own entry, where it has one.
pub(crate) async fn own_rate(
    conn: &mut Conn,
    provider: Id,
    model: &str,
) -> Result<Option<Rate>, EngineError> {
    Ok(resources::model(conn, provider, model)
        .await?
        .and_then(|entry| serde_json::from_value::<ModelParameters>(entry.parameters).ok())
        .and_then(|parameters| parameters.rate))
}

/// What a turn that is under way has reported that the ledger does not hold yet, in the cap's unit.
async fn progress(
    conn: &mut Conn,
    cap: &CapRow,
    turn: &rows::Running,
) -> Result<Micros, EngineError> {
    let Some(reported) = turn.usage.as_ref().map(tokens_of) else {
        return Ok(Micros::ZERO);
    };
    if cap.meter == meter::TOKENS {
        return Ok(Micros::whole(
            reported.total.saturating_sub(turn.counted_tokens).max(0),
        ));
    }
    let own = own_rate(conn, turn.provider_id, &turn.model).await?;
    let book = RateBook::load(conn, turn.thread_id).await?;
    let priced = book
        .price(
            conn,
            own.as_ref(),
            &turn.model,
            &reported,
            Some(reported.input),
            None,
        )
        .await?;
    let reported = priced
        .credits
        .as_deref()
        .and_then(Micros::parse)
        .unwrap_or(Micros::ZERO);
    let counted = Micros::parse(&turn.counted_credits).unwrap_or(Micros::ZERO);
    Ok(reported.floor_sub(counted))
}

/// The service tier a running turn asked for: the value of the parameter its harness declares as the
/// model's service tiers, among the parameters it was started with.
fn tier_of_running(turn: &rows::Running) -> Option<String> {
    let declared: Vec<ParameterDef> = turn
        .declared
        .clone()
        .and_then(|declared| serde_json::from_value(declared).ok())
        .unwrap_or_default();
    crate::providers::service_tier(&declared, &turn.params)
}

/// Of the turns under way that a cap covers by its node, provider, harness kind and subject, those whose
/// model and service tier it names as well.
async fn covered_running(
    conn: &mut Conn,
    cap: &CapRow,
    turns: Vec<rows::Running>,
) -> Result<Vec<rows::Running>, EngineError> {
    let groups = groups_for(
        conn,
        cap,
        turns
            .iter()
            .map(|turn| (turn.provider_id, turn.model.clone()))
            .collect(),
    )
    .await?;
    Ok(turns
        .into_iter()
        .filter(|turn| {
            covers_model(
                cap,
                &turn.model,
                groups_in(&groups, turn.provider_id, &turn.model),
            ) && covers_tier(cap, tier_of_running(turn).as_deref())
        })
        .collect())
}

/// How a cap stands for the person it is counted for at `now`.
pub async fn standing(
    conn: &mut Conn,
    cap: &CapRow,
    person: Option<Id>,
    now: DateTime<Utc>,
) -> Result<Standing, EngineError> {
    let amount = size(cap);
    let person = person_for(cap, person);
    let window = window(cap, now);
    let under_way = rows::running(conn, cap, person, now).await?;
    let running = covered_running(conn, cap, under_way).await?;
    if is_running(cap) {
        return Ok(Standing {
            cap: cap.clone(),
            person,
            amount,
            used: Micros::whole(i64::try_from(running.len()).unwrap_or(i64::MAX)),
            in_progress: Micros::ZERO,
            window,
            unrated_tokens: 0,
        });
    }
    let (mut tokens, mut credits, mut unrated) = (0_i64, Micros::ZERO, 0_i64);
    let used = rows::used(conn, cap, person, window.start, now).await?;
    let groups = groups_for(
        conn,
        cap,
        used.iter()
            .map(|row| (row.provider_id, row.model.clone()))
            .collect(),
    )
    .await?;
    for row in used.iter().filter(|row| {
        covers_model(
            cap,
            &row.model,
            groups_in(&groups, row.provider_id, &row.model),
        )
    }) {
        tokens = tokens.saturating_add(row.total_tokens);
        credits += Micros::parse(&row.credits).unwrap_or(Micros::ZERO);
        unrated = unrated.saturating_add(row.unrated_tokens);
    }
    let mut in_progress = Micros::ZERO;
    for turn in &running {
        in_progress += progress(conn, cap, turn).await?;
    }
    Ok(Standing {
        cap: cap.clone(),
        person,
        amount,
        used: if cap.meter == meter::TOKENS {
            Micros::whole(tokens)
        } else {
            credits
        },
        in_progress,
        window,
        unrated_tokens: if cap.meter == meter::CREDITS {
            unrated
        } else {
            0
        },
    })
}

/// The role the person holds at the node, where it matters: for a cap for a role.
async fn role_of(
    conn: &mut Conn,
    person: Id,
    node: Id,
    now: DateTime<Utc>,
) -> Result<Option<Role>, EngineError> {
    match Principal::load(conn, Caller::session(person), Time::new(now)).await {
        Ok(principal) => Ok(orch_authz::caps::role_at(conn, &principal, node).await?),
        Err(orch_authz::Error::NoSuchUser(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The caps that cover the turn, each with the cap that applies in its stead where there is one.
pub async fn covering(
    conn: &mut Conn,
    ask: &Ask,
    now: DateTime<Utc>,
) -> Result<Vec<Applied>, EngineError> {
    let found = rows::covering(
        conn,
        &ask.path,
        ask.provider,
        TierAsked::of_turn(ask.tier.as_deref()),
        ask.harness_kind.as_deref(),
        ask.person,
        now,
    )
    .await?;
    let role = match ask.person {
        Some(person) if found.iter().any(|cap| cap.subject_kind == subject::ROLE) => {
            role_of(conn, person, ask.thread, now).await?
        }
        _ => None,
    };
    let named_groups = if found.iter().any(names_a_group) {
        orch_db::queries::resources::groups_of(conn, &[(ask.provider, ask.model.clone())])
            .await?
            .remove(&(ask.provider, ask.model.clone()))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let covers = found
        .into_iter()
        .filter(|cap| covers_model(cap, &ask.model, &named_groups))
        .filter(|cap| !(cap.per_person() && ask.person.is_none()))
        .filter(|cap| {
            cap.subject_kind != subject::ROLE
                || cap.subject_id.is_some()
                || cap
                    .subject_role
                    .as_deref()
                    .and_then(|word| word.parse::<Role>().ok())
                    .is_some_and(|wanted| role.is_some_and(|held| held.includes(wanted)))
        })
        .collect();
    Ok(precedence(covers))
}

/// Whether a cap's subject covers a person: everybody; that person; a group they are in; or the role they
/// hold at `place`, the node where the work is, including that role and higher roles there.
/// Without a place the role is looked for at the node the cap names, and where it
/// names none, at the orgs the person belongs to.
pub async fn covers_person(
    conn: &mut Conn,
    cap: &CapRow,
    person: Id,
    place: Option<Id>,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    Ok(match cap.subject_kind.as_str() {
        subject::EVERYONE => true,
        subject::USER => cap.subject_id == Some(person),
        subject::MEMBERS => match cap.subject_id {
            Some(org) => sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM access_holders($1, $2, false) WHERE kind = 'members' AND id = $3)")
                .bind(person).bind(now).bind(org).fetch_one(conn).await.map_err(orch_db::DbError::from)?,
            None => false,
        },
        subject::GROUP => match cap.subject_id {
            Some(group) => rows::in_group(conn, group, person, now).await?,
            None => false,
        },
        subject::ROLE => {
            let Some(wanted) = cap
                .subject_role
                .as_deref()
                .and_then(|word| word.parse::<Role>().ok())
            else {
                return Ok(false);
            };
            match cap.subject_id.or(place).or(cap.node_id) {
                Some(node) if wanted == Role::Viewer && cap.subject_id.is_some() => {
                    Principal::load(conn, Caller::session(person), Time::new(now)).await?.is_viewer_of(node)
                }
                Some(node) => role_of(conn, person, node, now).await?.is_some_and(|held| held.includes(wanted)),
                None => {
                    let mut held = false;
                    for org in hierarchy::root_orgs_of(conn, person).await? {
                        if role_of(conn, person, org, now).await?.is_some_and(|held| held.includes(wanted)) {
                            held = true;
                            break;
                        }
                    }
                    held
                }
            }
        }
        _ => false,
    })
}

/// Where a cap is defined, as the access engine names it.
#[must_use]
pub fn home_of(cap: &CapRow) -> Home {
    Home::of(cap.node_id, cap.provider_id)
}

/// A cap's place in a sentence: `project "Web"`, `provider "Key"`, `the system`.
async fn place_label(conn: &mut Conn, cap: &CapRow) -> Result<String, EngineError> {
    let node = match cap.node_id {
        Some(node) => hierarchy::with_ancestors(conn, &[node])
            .await?
            .into_iter()
            .find(|each| each.id == node)
            .map(|found| format!("{} \"{}\"", found.kind, found.name)),
        None => None,
    };
    let provider = match cap.provider_id {
        Some(provider) => rows::provider_name(conn, provider)
            .await?
            .map(|name| format!("provider \"{name}\"")),
        None => None,
    };
    Ok(match (provider, node) {
        (Some(provider), Some(node)) => format!("{provider} at {node}"),
        (Some(place), None) | (None, Some(place)) => place,
        (None, None) => "the system".to_owned(),
    })
}

/// A whole amount with its thousands told apart, a fraction as it is.
fn shown(amount: Micros) -> String {
    let text = amount.text();
    let (whole, fraction) = text
        .split_once('.')
        .map_or((text.as_str(), ""), |(whole, fraction)| (whole, fraction));
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(' ');
        }
        grouped.push(digit);
    }
    if fraction.is_empty() {
        grouped
    } else {
        format!("{grouped}.{fraction}")
    }
}

/// The sentence that says what stands in the way of a turn, and when it ends.
async fn describe(conn: &mut Conn, standing: &Standing) -> Result<String, EngineError> {
    let place = place_label(conn, &standing.cap).await?;
    let amount = standing.amount.unwrap_or(Micros::ZERO);
    if is_running(&standing.cap) {
        return Ok(format!(
            "The cap of {} turns at once on {place} is full: {} running.",
            shown(amount),
            shown(standing.used)
        ));
    }
    let adjective = match standing.cap.period.as_deref() {
        Some(period::DAY) => "Daily",
        Some(period::WEEK) => "Weekly",
        Some(period::MONTH) => "Monthly",
        _ => "Total",
    };
    let unit = if standing.cap.meter == meter::TOKENS {
        "tokens"
    } else {
        "credits"
    };
    let again = standing.window.ends_at.map_or_else(String::new, |at| {
        format!(" Starts again {}.", at.format("%Y-%m-%dT%H:%MZ"))
    });
    Ok(format!(
        "{adjective} cap of {place} reached: {} of {} {unit}.{again}",
        shown(standing.counted()),
        shown(amount)
    ))
}

// ---------------------------------------------------------------- what the API shows of a cap

/// A cap as the screens show it: how it stands, who replaces it, how many wait for it and how often a turn
/// was let past it.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// How it stands for the person it is counted for.
    pub standing: Standing,
    /// For a cap counted per person: the cap that applies to this person instead.
    pub replaced_by: Option<Id>,
    /// For a cap on running turns: the threads and held inputs that wait for room in it.
    pub waiting: i64,
    /// For a cap in credits or tokens: the covered threads in which nobody reports usage.
    pub unmetered_threads: i64,
    /// How often turns were let past it, and the latest.
    pub bypassed: BypassTally,
}

/// How a cap stands now, with what a screen adds to it.
pub async fn status(
    conn: &mut Conn,
    cap: &CapRow,
    person: Option<Id>,
    replaced_by: Option<Id>,
    now: DateTime<Utc>,
) -> Result<Status, EngineError> {
    let standing = standing(conn, cap, person, now).await?;
    let since = if is_running(cap) {
        now - RUNNING_LOOK_BACK
    } else {
        standing
            .window
            .start
            .and_then(midnight)
            .unwrap_or(DateTime::UNIX_EPOCH)
    };
    let waiting = if is_running(cap) {
        i64::try_from(waiting_for(conn, cap, now).await?.len()).unwrap_or(i64::MAX)
    } else {
        0
    };
    Ok(Status {
        replaced_by,
        waiting,
        unmetered_threads: if is_running(cap) {
            0
        } else {
            rows::unmetered_threads(conn, cap).await?
        },
        bypassed: rows::bypasses_of(conn, cap.id, since).await?,
        standing,
    })
}

/// What waits for room in a cap, in the order it will run: the threads blocked and the inputs held for want
/// of room, but only those the cap has no room for now, since a thread that another cap blocks is not
/// waiting for this one. Coverage uses the model and service tier its next turn would run with.
pub async fn waiting_for(
    conn: &mut Conn,
    cap: &CapRow,
    now: DateTime<Utc>,
) -> Result<Vec<rows::Waiter>, EngineError> {
    if !cap.enabled || size(cap).is_none() {
        return Ok(Vec::new());
    }
    let candidates = rows::waiting(conn, cap, now).await?;
    let mut asks = HashMap::new();
    for waiter in &candidates {
        if asks.contains_key(&waiter.thread_id) {
            continue;
        }
        let ask = match threads::thread(conn, waiter.thread_id).await? {
            Some(thread) => {
                let settings = crate::resolved::load(conn, waiter.thread_id).await?;
                ask_from_settings(conn, &thread, &settings).await?
            }
            None => None,
        };
        asks.insert(waiter.thread_id, ask);
    }
    let groups = groups_for(
        conn,
        cap,
        asks.values()
            .flatten()
            .map(|ask| (ask.provider, ask.model.clone()))
            .collect(),
    )
    .await?;
    let mut waiters = Vec::new();
    for waiter in candidates {
        let Some(ask) = asks.get(&waiter.thread_id).and_then(Option::as_ref) else {
            continue;
        };
        if !covers_model(
            cap,
            &ask.model,
            groups_in(&groups, ask.provider, &ask.model),
        ) || !covers_tier(cap, ask.tier.as_deref())
        {
            continue;
        }
        if cap.subject_kind == subject::ROLE {
            let Some(person) = waiter.author_id else {
                continue;
            };
            if !covers_person(conn, cap, person, Some(waiter.thread_id), now).await? {
                continue;
            }
        }
        waiters.push(waiter);
    }
    if waiters.is_empty() {
        return Ok(waiters);
    }
    if !cap.per_person() {
        let standing = standing(conn, cap, None, now).await?;
        return Ok(if standing.has_room() {
            Vec::new()
        } else {
            waiters
        });
    }
    let mut reached: BTreeMap<Id, bool> = BTreeMap::new();
    let mut kept = Vec::new();
    for waiter in waiters {
        let Some(author) = waiter.author_id else {
            continue;
        };
        let full = match reached.get(&author) {
            Some(full) => *full,
            None => {
                let full = !standing(conn, cap, Some(author), now).await?.has_room();
                reached.insert(author, full);
                full
            }
        };
        if full {
            kept.push(waiter);
        }
    }
    Ok(kept)
}

// ---------------------------------------------------------------- before a turn starts

/// A limit a turn was let past, with who let it past and what it showed then.
#[derive(Debug, Clone, PartialEq)]
pub struct Passed {
    /// How the cap stood.
    pub standing: Standing,
    /// Who let the turn past it.
    pub by: Id,
}

/// A turn that starts past the limit of its provider's own plan, and who let it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderPass<'a> {
    /// The thread.
    pub thread: Id,
    /// The segment the turn is in.
    pub segment: Id,
    /// The turn.
    pub turn: Id,
    /// Whose input it is.
    pub author: Option<Id>,
    /// The provider whose limit it was.
    pub provider: Id,
    /// Who let it past.
    pub by: Id,
    /// What the limit showed.
    pub detail: &'a str,
}

/// What the caps say of a turn that would start.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Every cap has room, or was passed.
    Clear,
    /// A cap counted per person has no room for the author: their input is held.
    Held(String),
    /// A cap has no room for everybody, or cannot measure the turn: the thread is blocked.
    Blocked(String),
}

/// The answer of the caps for a turn that would start.
#[derive(Debug, Clone, PartialEq)]
pub struct CapCheck {
    /// Whether it may start.
    pub verdict: Verdict,
    /// The input's bypass as it stands after the entries that no longer hold are dropped.
    pub bypass: Option<Json>,
    /// Whether that differs from what the input carries.
    pub bypass_changed: bool,
    /// The limits the turn is let past, if it starts.
    pub passed: Vec<Passed>,
    /// What the lease may carry, if the turn starts: the room left of the tightest cap that would stop it.
    pub room: Room,
    /// The cap that blocks the thread, where the verdict is that it is blocked.
    pub blocked_by: Option<Id>,
}

/// What a turn may still use before a cap that would stop it is reached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Room {
    /// In tokens.
    pub tokens: Option<i64>,
    /// In credits.
    pub credits: Option<Micros>,
}

impl Engine {
    /// Whether the person may let the input past the cap now: the right is read again, each time.
    async fn may_pass_now(
        &self,
        conn: &mut Conn,
        by: Id,
        as_admin: bool,
        cap: &CapRow,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        let caller = Caller::session(by).with_admin_mode(as_admin);
        let principal = match Principal::load(conn, caller, Time::new(now)).await {
            Ok(principal) => principal,
            Err(orch_authz::Error::NoSuchUser(_)) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        Ok(orch_authz::caps::may_pass(conn, &principal, home_of(cap), Policy::default()).await?)
    }

    /// Asks the caps whether the turn that the input would start may start: the check of the design that
    /// comes last in pre-flight. Run it in the transaction that makes the turn, so that the locks it takes on
    /// caps for turns at once hold until the turn is there to be counted.
    pub(crate) async fn check_caps(
        &self,
        unit: &mut Unit<'_>,
        input: &InputRow,
        ask: &Ask,
    ) -> Result<CapCheck, EngineError> {
        let now = unit.now();
        let applied = covering(&mut *unit.conn, ask, now).await?;
        let mut limited: Vec<&Applied> = applied
            .iter()
            .filter(|each| each.replaced_by.is_none() && each.cap.amount.is_some())
            .collect();
        // Turns running at once come first, then tokens and credits.
        limited.sort_by_key(|each| (!is_running(&each.cap), each.cap.id));
        let running: Vec<Id> = limited
            .iter()
            .filter(|each| is_running(&each.cap))
            .map(|each| each.cap.id)
            .collect();
        rows::lock_for_counting(&mut *unit.conn, &running).await?;
        let mut standings: Vec<Standing> = Vec::new();
        for each in &limited {
            standings.push(standing(&mut *unit.conn, &each.cap, ask.person, now).await?);
        }
        self.note_standings(unit, &standings).await?;

        // An input that no person sent is never let past a limit.
        let entries = if input.author_id.is_some() {
            entries_of(input.limit_bypass.as_ref())
        } else {
            Vec::new()
        };
        let mut kept: Vec<Entry> = entries
            .iter()
            .map(|entry| Entry {
                caps: Vec::new(),
                ..entry.clone()
            })
            .collect();
        let mut passed: Vec<Passed> = Vec::new();
        let mut shared: Vec<&Standing> = Vec::new();
        let mut own: Vec<&Standing> = Vec::new();
        for each in &standings {
            if each.has_room() {
                continue;
            }
            let mut passer = None;
            for (index, entry) in entries.iter().enumerate() {
                if entry.caps.contains(&each.cap.id)
                    && self
                        .may_pass_now(&mut *unit.conn, entry.by, entry.as_admin, &each.cap, now)
                        .await?
                {
                    kept[index].caps.push(each.cap.id);
                    passer = Some(entry.by);
                    break;
                }
            }
            match passer {
                Some(by) => passed.push(Passed {
                    standing: each.clone(),
                    by,
                }),
                None if each.cap.per_person() => own.push(each),
                None => shared.push(each),
            }
        }

        // What a cap cannot measure is refused by a cap that says so.
        let mut unmeasured: Vec<&Standing> = Vec::new();
        let mut unrated = None;
        for each in &standings {
            if is_running(&each.cap)
                || each.cap.uncounted != uncounted::BLOCK
                || passed
                    .iter()
                    .any(|pass| pass.standing.cap.id == each.cap.id)
            {
                continue;
            }
            let blind = ask.metering == Some(Metering::None);
            let no_rate = if each.cap.meter == meter::CREDITS && !blind {
                if unrated.is_none() {
                    let own = own_rate(&mut *unit.conn, ask.provider, &ask.model).await?;
                    let book = RateBook::load(&mut *unit.conn, ask.thread).await?;
                    unrated = Some(
                        book.find(&mut *unit.conn, own.as_ref(), &ask.model)
                            .await?
                            .is_none(),
                    );
                }
                unrated == Some(true)
            } else {
                false
            };
            if blind || no_rate {
                unmeasured.push(each);
            }
        }

        let bypass = entries_json(&kept);
        let bypass_changed = bypass != input.limit_bypass;
        let blocked_by = shared
            .first()
            .map(|each| each.cap.id)
            .or_else(|| unmeasured.first().map(|each| each.cap.id));
        let verdict = if let Some(first) = shared.first() {
            Verdict::Blocked(describe(&mut *unit.conn, first).await?)
        } else if let Some(first) = unmeasured.first() {
            let what = if ask.metering == Some(Metering::None) {
                "nobody reports what this turn uses"
            } else {
                "the model has no rate"
            };
            Verdict::Blocked(format!(
                "{} cannot count this turn: {what}, and the cap says to hold it back.",
                capitalised(&place_label(&mut *unit.conn, &first.cap).await?)
            ))
        } else if let Some(first) = own.first() {
            Verdict::Held(describe(&mut *unit.conn, first).await?)
        } else {
            Verdict::Clear
        };
        let room = if verdict == Verdict::Clear {
            room_left(&standings, &passed)
        } else {
            Room::default()
        };
        let blocked_by = matches!(verdict, Verdict::Blocked(_))
            .then_some(blocked_by)
            .flatten();
        Ok(CapCheck {
            verdict,
            bypass,
            bypass_changed,
            passed,
            room,
            blocked_by,
        })
    }

    /// Looks again at the inputs of a thread that were held for their author's own cap: those whose author's
    /// caps have room now, or were let past by somebody who holds the row, are released and take their place.
    /// Says whether any was.
    pub(crate) async fn release_held_inputs(
        &self,
        unit: &mut Unit<'_>,
        asking: &Ask,
        pending: &[InputRow],
    ) -> Result<bool, EngineError> {
        let mut released = false;
        for input in pending
            .iter()
            .filter(|input| input.state_detail.as_deref() == Some(threads::HELD_CAP_REACHED))
        {
            let ask = Ask {
                person: input.answers(),
                ..asking.clone()
            };
            let check = self.check_caps(unit, input, &ask).await?;
            if !matches!(check.verdict, Verdict::Held(_)) {
                let seq = unit.stamp(input.thread_id).await?;
                threads::hold_input(&mut *unit.conn, input.id, None, seq).await?;
                released = true;
            }
        }
        Ok(released)
    }

    /// Writes down, for a turn that has started past limits, what each was: a row, a line of the audit
    /// trail and a marker in the thread, for every limit and every person who let it past.
    pub(crate) async fn record_passed(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        segment: Id,
        turn: Id,
        author: Option<Id>,
        passed: &[Passed],
    ) -> Result<(), EngineError> {
        let now = unit.now();
        let mut by_person: BTreeMap<Id, Vec<&Passed>> = BTreeMap::new();
        for each in passed {
            by_person.entry(each.by).or_default().push(each);
        }
        let org = rows::node_org(&mut *unit.conn, thread).await?;
        for (by, limits) in by_person {
            let mut shown_limits = Vec::new();
            for each in limits {
                let standing = &each.standing;
                let used = standing.counted().text();
                let amount = standing.amount.map(Micros::text);
                rows::record_bypass(
                    &mut *unit.conn,
                    &NewBypass {
                        id: new_id(),
                        cap_id: Some(standing.cap.id),
                        provider_id: None,
                        thread_id: thread,
                        turn_id: turn,
                        author_id: author,
                        by_user: by,
                        meter: &standing.cap.meter,
                        used: Some(&used),
                        amount: amount.as_deref(),
                        period_start: standing.window.start,
                        at: now,
                    },
                )
                .await?;
                let label = describe(&mut *unit.conn, standing).await?;
                shown_limits.push(json!({
                    "cap": standing.cap.id, "label": label, "used": used, "amount": amount,
                }));
                let action = audit::action!(AuditAction::UsageLimitBypassed);
                let summary = format!("A turn was let past a limit that was reached: {label}");
                let mut entry = audit::Entry::new(
                    Actor::User(by),
                    action.as_str(),
                    action.object_kind().unwrap_or("usage"),
                    Some(thread),
                    &summary,
                );
                entry.history = action.is_history_line();
                entry.node_id = Some(thread);
                entry.org_id = org;
                entry.details = json!({
                    "turn_id": turn, "cap_id": standing.cap.id, "meter": standing.cap.meter,
                    "used": used, "amount": amount, "author_id": author,
                });
                audit::append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
            }
            let person = crate::views::user_ref_json(&mut *unit.conn, by).await?;
            ingest::add_marker(
                unit,
                thread,
                segment,
                Some(turn),
                &json!({ "marker": "limit_bypassed", "data": { "by": person, "turn_id": turn, "limits": shown_limits } }),
                &format!("limit_bypassed:{turn}:{by}"),
            )
            .await?;
        }
        // Several reached caps, or several authorizations, still let the same turn past once per meter.
        for meter in passed
            .iter()
            .map(|pass| pass.standing.cap.meter.clone())
            .collect::<std::collections::BTreeSet<_>>()
        {
            metrics::counter!("orch_limit_bypasses_total","meter"=>meter).increment(1);
        }
        Ok(())
    }

    /// Writes down that a turn was let past the provider's own limit, by whom.
    pub(crate) async fn record_passed_provider(
        &self,
        unit: &mut Unit<'_>,
        pass: &ProviderPass<'_>,
    ) -> Result<(), EngineError> {
        let ProviderPass {
            thread,
            segment,
            turn,
            author,
            provider,
            by,
            detail,
        } = *pass;
        let now = unit.now();
        rows::record_bypass(
            &mut *unit.conn,
            &NewBypass {
                id: new_id(),
                cap_id: None,
                provider_id: Some(provider),
                thread_id: thread,
                turn_id: turn,
                author_id: author,
                by_user: by,
                meter: "provider_limit",
                used: None,
                amount: None,
                period_start: None,
                at: now,
            },
        )
        .await?;
        let org = rows::node_org(&mut *unit.conn, thread).await?;
        let action = AuditAction::UsageLimitBypassed;
        let summary = format!("A turn was let past the limit of its provider's plan: {detail}");
        let mut entry = audit::Entry::new(
            Actor::User(by),
            action.as_str(),
            action.object_kind().unwrap_or("usage"),
            Some(thread),
            &summary,
        );
        entry.history = action.is_history_line();
        entry.node_id = Some(thread);
        entry.org_id = org;
        entry.details =
            json!({ "turn_id": turn, "provider_id": provider, "meter": "provider_limit" });
        audit::append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
        let person = crate::views::user_ref_json(&mut *unit.conn, by).await?;
        ingest::add_marker(
            unit,
            thread,
            segment,
            Some(turn),
            &json!({ "marker": "limit_bypassed", "data": { "by": person, "turn_id": turn, "limits": [{
                "cap": Json::Null, "label": detail, "used": "0", "amount": Json::Null,
            }] } }),
            &format!("limit_bypassed:{turn}:{by}:provider"),
        )
        .await?;
        metrics::counter!("orch_limit_bypasses_total","meter"=>"provider_limit").increment(1);
        Ok(())
    }
}

fn capitalised(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + letters.as_str()
    })
}

/// The least room left among the caps that would stop the turn and that it was not let past.
fn room_left(standings: &[Standing], passed: &[Passed]) -> Room {
    let mut room = Room::default();
    for each in standings.iter().filter(|each| {
        !is_running(&each.cap)
            && each.cap.at_limit == at_limit::STOP
            && !passed
                .iter()
                .any(|pass| pass.standing.cap.id == each.cap.id)
    }) {
        let Some(left) = each.room_left() else {
            continue;
        };
        if each.cap.meter == meter::TOKENS {
            let tokens = left.to_whole().unwrap_or(i64::MAX);
            room.tokens = Some(room.tokens.map_or(tokens, |known| known.min(tokens)));
        } else {
            room.credits = Some(room.credits.map_or(left, |known| known.min(left)));
        }
    }
    room
}

impl Engine {
    /// The allowance a lease carries for the room a turn has, with the rate its requests are counted at
    /// where a cap in credits is among them.
    pub(crate) async fn allowance_of(
        &self,
        conn: &mut Conn,
        room: &Room,
        ask: &Ask,
    ) -> Result<Option<Json>, EngineError> {
        let credits = match room.credits {
            Some(left) => {
                let own = own_rate(conn, ask.provider, &ask.model).await?;
                let book = RateBook::load(conn, ask.thread).await?;
                // The forwarder does not know the tier a turn runs with: the rate it is given is
                // the one the tier comes to.
                book.find(conn, own.as_ref(), &ask.model)
                    .await?
                    .and_then(|found| crate::rates::for_tier(&found.rate, ask.tier.as_deref()))
                    .map(|rate| (left, rate))
            }
            None => None,
        };
        Ok(crate::leases::allowance_for(
            room.tokens,
            credits.as_ref().map(|(left, rate)| (*left, rate)),
        ))
    }
}

impl Engine {
    /// Whether Orch may ask a model for a summary of the topic on the provider: every shared cap that
    /// covers it has room. A summary is a model call and no turn, so a cap on turns at once does not hold it
    /// back, a cap counted per person does not cover it, and it never blocks or delays a turn: when a cap has
    /// no room it is skipped.
    pub async fn room_for_summary(
        &self,
        conn: &mut Conn,
        topic: Id,
        provider: Id,
        model: &str,
    ) -> Result<bool, EngineError> {
        let now = self.inner.clock.now();
        let ask = Ask {
            thread: topic,
            path: threads::node_path(conn, topic).await?,
            provider,
            model: model.to_owned(),
            tier: None,
            harness_kind: None,
            person: None,
            metering: None,
        };
        for each in covering(conn, &ask, now).await? {
            if each.replaced_by.is_some()
                || each.cap.amount.is_none()
                || each.cap.per_person()
                || is_running(&each.cap)
            {
                continue;
            }
            if !standing(conn, &each.cap, None, now).await?.has_room() {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

// ---------------------------------------------------------------- notices

impl Engine {
    /// Tells the people a cap concerns that its use passed the warning percent or that it was reached, once for
    /// each cap, person and period; a cap that was reached is audited.
    async fn note_standings(
        &self,
        unit: &mut Unit<'_>,
        standings: &[Standing],
    ) -> Result<(), EngineError> {
        for each in standings {
            if is_running(&each.cap) {
                continue;
            }
            let (word, kind) = match each.state() {
                State::Ok => continue,
                State::Warning => (level::WARNING, WARNING_KIND),
                State::Reached => (level::REACHED, REACHED_KIND),
            };
            let now = unit.now();
            let start = period_key(&each.cap, &each.window);
            if !rows::note_once(&mut *unit.conn, each.cap.id, each.person, start, word, now).await?
            {
                continue;
            }
            let sentence = describe(&mut *unit.conn, each).await?;
            let org = match each.cap.node_id {
                Some(node) => rows::node_org(&mut *unit.conn, node).await?,
                None => None,
            };
            if word == level::REACHED {
                let action = audit::action!(AuditAction::UsageCapReached);
                let mut entry = audit::Entry::new(
                    Actor::System,
                    action.as_str(),
                    action.object_kind().unwrap_or("usage_cap"),
                    Some(each.cap.id),
                    &sentence,
                );
                entry.node_id = each.cap.node_id;
                entry.org_id = org;
                entry.details = json!({
                    "cap_id": each.cap.id, "person": each.person, "period_start": start,
                    "used": each.counted().text(), "amount": each.amount.map(Micros::text),
                });
                audit::append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
            }
            let mut told: Vec<Id> = Vec::new();
            if each.cap.per_person() {
                told.extend(each.person);
            }
            if word == level::REACHED || !each.cap.per_person() {
                let managers =
                    orch_authz::caps::managers(&mut *unit.conn, home_of(&each.cap), Time::new(now))
                        .await?;
                told.extend(managers);
            }
            told.sort_unstable();
            told.dedup();
            let title = if word == level::REACHED {
                "A usage cap was reached"
            } else {
                "A usage cap is nearly used up"
            };
            let mail_shared = orch_settings::get::<bool>(
                &mut *unit.conn,
                orch_settings::Place::System,
                "mail.shared",
            )
            .await?
            .unwrap_or(false);
            for user in told {
                let data = json!({
                    "cap_id": each.cap.id, "person": each.person, "period_start": start,
                    "path": "/usage",
                });
                let dedupe = format!(
                    "{kind}:{}:{}:{start}",
                    each.cap.id,
                    each.person.map_or_else(String::new, |id| id.to_string())
                );
                let Some(announced) = people::announce(
                    &mut *unit.conn,
                    &people::NewNotification {
                        user_id: user,
                        kind,
                        node_id: each.cap.node_id,
                        thread_id: None,
                        approval_id: None,
                        chat_id: None,
                        title,
                        body: &sentence,
                        data: &data,
                        dedupe_key: &dedupe,
                        at: now,
                    },
                )
                .await?
                else {
                    continue;
                };
                let snapshot = Snapshot {
                    id: Some(announced.id),
                    kind: kind.to_owned(),
                    at: now.into(),
                    title: title.to_owned(),
                    body: sentence.clone(),
                    node_id: each.cap.node_id,
                    thread_id: None,
                    approval_id: None,
                    chat_id: None,
                    data,
                };
                routes::tell_routes(&mut *unit.conn, user, &snapshot, org, mail_shared, now)
                    .await?;
            }
        }
        Ok(())
    }
}

impl Engine {
    /// What a thread's next turn would be asked of the caps by, as far as the thread says: its provider, its
    /// model as the settings resolve it, the harness kind of its agent. Without one of them no cap covers it.
    pub(crate) async fn ask_of_thread(
        &self,
        unit: &mut Unit<'_>,
        thread: &threads::ThreadRow,
        settings: &crate::resolved::Resolved,
    ) -> Result<Option<Ask>, EngineError> {
        ask_from_settings(&mut *unit.conn, thread, settings).await
    }
}

/// The same next-turn coverage for preflight, the waiting display, and queue ordering.
async fn ask_from_settings(
    conn: &mut Conn,
    thread: &threads::ThreadRow,
    settings: &crate::resolved::Resolved,
) -> Result<Option<Ask>, EngineError> {
    let (Some(provider), Some(agent), Some(model)) =
        (thread.provider_id, thread.agent_id, settings.model.clone())
    else {
        return Ok(None);
    };
    let Some(agent) = resources::agent(conn, agent).await? else {
        return Ok(None);
    };
    // The tier the next turn would ask for, as far as the thread says: what it chose, if that is open on
    // the model, and otherwise what the model starts with.
    let entry = resources::model(conn, provider, &model).await?;
    let tier = match (entry, crate::harnesses::descriptor_of(&agent)?) {
        (Some(entry), Some(descriptor)) => {
            let parameters =
                serde_json::from_value::<ModelParameters>(entry.parameters).unwrap_or_default();
            tier_asked(&descriptor.parameters, &parameters, &settings.params)
        }
        _ => None,
    };
    Ok(Some(Ask {
        thread: thread.node_id,
        path: threads::node_path(conn, thread.node_id).await?,
        provider,
        model,
        tier,
        harness_kind: Some(agent.harness_kind),
        person: None,
        metering: None,
    }))
}

/// What the thread's next turn would ask of caps for this input author, resolved from its current settings.
pub async fn ask_for_thread(
    conn: &mut Conn,
    thread: Id,
    person: Option<Id>,
) -> Result<Option<Ask>, EngineError> {
    let Some(row) = threads::thread(conn, thread).await? else {
        return Ok(None);
    };
    let settings = crate::resolved::load(conn, thread).await?;
    let mut ask = ask_from_settings(conn, &row, &settings).await?;
    if let Some(ask) = &mut ask {
        ask.person = person;
    }
    Ok(ask)
}

/// The service tier a turn of a thread would ask for from what the thread chose (`stored`, the resolved
/// `agent.params`) and what the model's entry says: the choice, if it is one of the tiers enabled on the
/// model, and otherwise what the model starts with. None is the ordinary tier.
fn tier_asked(declared: &[ParameterDef], model: &ModelParameters, stored: &Json) -> Option<String> {
    use orch_agent_contract::parameters::ThreadKind;
    let def = declared
        .iter()
        .find(|def| def.kind_of_thread() == Some(ThreadKind::ServiceTier))?;
    stored
        .get(&def.key)
        .and_then(Json::as_str)
        .filter(|chosen| model.service_tiers().iter().any(|tier| tier.id == *chosen))
        .or_else(|| model.service_tier_start())
        .map(str::to_owned)
}

// ---------------------------------------------------------------- while a turn runs

/// The facts a running turn is asked of the caps by.
async fn ask_of_turn(conn: &mut Conn, turn: &TurnRow) -> Result<Option<Ask>, EngineError> {
    let Some(segment) = threads::segment(conn, turn.segment_id).await? else {
        return Ok(None);
    };
    let person = threads::input(conn, turn.input_id)
        .await?
        .and_then(|input| input.answers());
    let agent = resources::agent(conn, segment.agent_id).await?;
    let tier = match &agent {
        Some(agent) => crate::harnesses::descriptor_of(agent)?.and_then(|descriptor| {
            crate::providers::service_tier(&descriptor.parameters, &turn.params)
        }),
        None => None,
    };
    let harness_kind = agent.map(|agent| agent.harness_kind);
    let path = threads::node_path(conn, turn.thread_id).await?;
    Ok(Some(Ask {
        thread: turn.thread_id,
        path,
        provider: segment.provider_id,
        model: turn.model.clone(),
        tier,
        harness_kind,
        person,
        metering: None,
    }))
}

impl Engine {
    /// After usage was reported on a stream: looks again at the caps of the turns it concerns, no more often
    /// for a thread than `[engine] cap_check_secs`. Notices are written in the transaction of the report; a
    /// cap that is reached and stops its turns has them stopped once that transaction has committed.
    pub(crate) async fn caps_after_usage(
        &self,
        unit: &mut Unit<'_>,
        stream: &Stream,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        let mut threads_to_look_at: Vec<Id> = Vec::new();
        for delivered in events {
            match (
                stream,
                Event::decode(&delivered.kind, delivered.body.clone()),
            ) {
                (Stream::Session { thread_id, .. }, Ok(Event::UsageUpdated(_))) => {
                    threads_to_look_at.push(*thread_id);
                }
                (Stream::Provider { .. }, Ok(Event::LeaseUsage(usage))) => {
                    if let Some(lease) = threads::lease(&mut *unit.conn, usage.lease_id).await? {
                        threads_to_look_at.push(lease.thread_id);
                    }
                }
                _ => {}
            }
        }
        threads_to_look_at.sort_unstable();
        threads_to_look_at.dedup();
        for thread in threads_to_look_at {
            self.look_at_caps(unit, thread).await?;
        }
        Ok(())
    }

    async fn look_at_caps(&self, unit: &mut Unit<'_>, thread: Id) -> Result<(), EngineError> {
        let now = unit.now();
        {
            let mut checked = lock(&self.inner.cap_checks);
            let wait = Duration::from_std(self.inner.config.cap_check)
                .unwrap_or_else(|_| Duration::zero());
            forget_stale_checks(&mut checked, now, wait);
            if checked.contains_key(&thread) {
                return Ok(());
            }
            checked.insert(thread, now);
        }
        self.look_at_caps_now(unit, thread).await
    }

    /// Forgets when a thread's caps were last looked at, once its turn ended: the next turn's are looked at
    /// afresh. A thread that was removed is forgotten by [`forget_stale_checks`] within one interval.
    pub(crate) fn forget_cap_check(&self, thread: Id) {
        lock(&self.inner.cap_checks).remove(&thread);
    }

    /// Looks at the caps of the turn a thread has under way, whenever it was looked at last.
    async fn look_at_caps_now(&self, unit: &mut Unit<'_>, thread: Id) -> Result<(), EngineError> {
        let now = unit.now();
        let Some(turn) = threads::active_turn(&mut *unit.conn, thread).await? else {
            return Ok(());
        };
        let Some(ask) = ask_of_turn(&mut *unit.conn, &turn).await? else {
            return Ok(());
        };
        let applied = covering(&mut *unit.conn, &ask, now).await?;
        let mut standings = Vec::new();
        for each in applied.iter().filter(|each| {
            each.replaced_by.is_none() && each.cap.amount.is_some() && !is_running(&each.cap)
        }) {
            standings.push(standing(&mut *unit.conn, &each.cap, ask.person, now).await?);
        }
        self.note_standings(unit, &standings).await?;
        for each in standings {
            if each.state() == State::Reached
                && each.cap.at_limit == at_limit::STOP
                && !bypass_names(turn.limit_bypass.as_ref(), each.cap.id)
            {
                self.stop_later(each.cap.id, each.person);
            }
        }
        Ok(())
    }

    /// Has the turns a cap covers stopped, once the transaction that showed it reached has committed.
    fn stop_later(&self, cap: Id, person: Option<Id>) {
        if !lock(&self.inner.cap_stops).insert((cap, person)) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(engine.inner.config.post_commit_grace).await;
            if let Err(error) = engine.stop_covered(cap, person).await {
                tracing::warn!(%error, %cap, "the turns a cap covers could not be stopped; the next report looks again");
            }
            lock(&engine.inner.cap_stops).remove(&(cap, person));
        });
    }

    /// Ends every running turn a cap that is reached covers (for a cap counted per person, the person's),
    /// except those it was let past.
    pub(crate) async fn stop_covered(
        &self,
        cap: Id,
        person: Option<Id>,
    ) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let (message, victims) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(cap) = rows::get(&mut conn, cap).await? else {
                return Ok(());
            };
            if !cap.enabled {
                return Ok(());
            }
            let standing = standing(&mut conn, &cap, person, now).await?;
            if standing.has_room() {
                return Ok(());
            }
            let message = describe(&mut conn, &standing).await?;
            let covered = rows::running(&mut conn, &cap, standing.person, now).await?;
            let victims: Vec<rows::Running> = covered_running(&mut conn, &cap, covered)
                .await?
                .into_iter()
                .filter(|turn| !bypass_names(turn.limit_bypass.as_ref(), cap.id))
                .collect();
            (message, victims)
        };
        for turn in victims {
            self.stop_turn(turn.thread_id, turn.turn_id, &message)
                .await?;
        }
        Ok(())
    }

    /// Ends a running turn because a cap was reached: its lease is closed, the harness is asked to stop, and
    /// the thread goes on to its next input, which the cap then holds or blocks.
    async fn stop_turn(
        &self,
        thread_id: Id,
        turn_id: Id,
        message: &str,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let (finished, machine) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
                return Ok(());
            };
            if !turn.is_active() {
                return Ok(());
            }
            // The turn is cut by a usage limit, not by a person (T8).
            threads::set_interrupt_cause(&mut *unit.conn, turn.id, "usage_limit").await?;
            ingest::add_error_item(
                &mut unit,
                thread_id,
                turn.segment_id,
                Some(turn.id),
                CAP_REACHED,
                message,
                false,
            )
            .await?;
            crate::leases::close_for_turn(&mut unit, thread_id, crate::leases::CAP_REACHED).await?;
            self.end_turn(
                &mut unit,
                &thread,
                &turn,
                TurnEnd {
                    state: turn_state::INTERRUPTED,
                    error: Some(error_json(CAP_REACHED, message, false)),
                    input: InputAfter::Keep,
                    then_block: None,
                },
            )
            .await?;
            let machine = self.harness_machine_of(&mut *unit.conn, thread_id).await?;
            (unit.finish().await?, machine)
        };
        self.commit(tx, finished).await?;
        if let Some(machine) = machine {
            self.send_interrupt_best_effort(machine, thread_id, turn_id)
                .await;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- waiting threads

impl Engine {
    /// The threads that wait for room in a cap, the one with the oldest input first.
    pub(crate) async fn threads_waiting_for_room(&self) -> Result<Vec<Id>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(rows::blocked_for_room(&mut conn, self.inner.config.sweep_batch).await?)
    }

    /// Looks at the threads that wait for room in a cap, one after the other, the one whose oldest input is
    /// oldest first, so that a cap behaves as a queue: when room is made, the first of those it fits starts.
    /// A pass under way is not started again.
    pub async fn release_waiting_threads(&self) -> Result<usize, EngineError> {
        if self
            .inner
            .releasing
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(0);
        }
        let outcome = self.release_in_order().await;
        self.inner
            .releasing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        outcome
    }

    async fn release_in_order(&self) -> Result<usize, EngineError> {
        let waiting = self.threads_waiting_for_room().await?;
        let mut started = 0;
        // The caps that a thread of this pass was found to wait for. Room may be made while the pass is
        // under way, and a thread behind one of those caps must not overtake the thread in front of it: it
        // is left for the next pass, which begins with the thread that has waited longest.
        let mut full: HashSet<Id> = HashSet::new();
        for thread in waiting {
            if self.is_stopped() {
                break;
            }
            if !full.is_empty() && self.is_behind(thread, &full).await? {
                continue;
            }
            lock(&self.inner.cap_waits).remove(&thread);
            match self.preflight(thread).await? {
                crate::preflight::Outcome::Started(_) => started += 1,
                crate::preflight::Outcome::Blocked(_) => {
                    if let Some(cap) = lock(&self.inner.cap_waits).remove(&thread) {
                        full.insert(cap);
                    }
                }
                _ => {}
            }
        }
        Ok(started)
    }

    /// Whether any of the full caps covers the thread's next model, tier and author: only that
    /// work waits behind an older input of those caps.
    async fn is_behind(&self, thread: Id, caps: &HashSet<Id>) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(row) = threads::thread(&mut conn, thread).await? else {
            return Ok(false);
        };
        let settings = crate::resolved::load(&mut conn, thread).await?;
        let Some(mut ask) = ask_from_settings(&mut conn, &row, &settings).await? else {
            return Ok(false);
        };
        let person = threads::pending_inputs(&mut conn, thread)
            .await?
            .into_iter()
            .find(|input| !input.is_held())
            .and_then(|input| input.answers());
        ask.person = person;
        let covering = covering(&mut conn, &ask, self.inner.clock.now()).await?;
        Ok(covering
            .iter()
            .any(|applied| applied.replaced_by.is_none() && caps.contains(&applied.cap.id)))
    }

    /// Notes which cap a thread waits for, for the pass that looks at the threads that wait.
    pub(crate) fn note_waiting_for(&self, thread: Id, cap: Id) {
        lock(&self.inner.cap_waits).insert(thread, cap);
    }

    /// Says that a cap was made, changed or removed: the turns it covers that are under way are looked at
    /// at once, as when their usage is reported (a cap that is reached and says `stop` ends them), and the
    /// threads that wait are let look again, since a cap raised, switched off or removed may have made room.
    /// A failure to look at a turn is for the next report of usage to put right.
    pub async fn cap_changed(&self, cap: Id) -> Result<(), EngineError> {
        let threads_under_way = {
            let mut conn = self.inner.db.acquire().await?;
            match rows::get(&mut conn, cap).await? {
                Some(cap) if cap.enabled => rows::running_threads(&mut conn, &cap).await?,
                _ => Vec::new(),
            }
        };
        for thread in threads_under_way {
            let mut tx = self.inner.db.begin().await?;
            let finished = {
                let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                self.look_at_caps_now(&mut unit, thread).await?;
                unit.finish().await?
            };
            self.commit(tx, finished).await?;
        }
        self.room_may_have_been_made();
        Ok(())
    }

    /// Says that room may have been made in a cap: a turn ended, a cap was raised or removed, a period turned.
    /// The supervisor looks at the threads that wait.
    pub fn room_may_have_been_made(&self) {
        *lock(&self.inner.room_made) = Some(self.inner.clock.now());
        self.inner.signal.notify_one();
    }

    /// Whether room was said to have been made a moment ago and has not been looked at yet.
    pub(crate) fn take_room_made(&self) -> bool {
        let mut made = lock(&self.inner.room_made);
        let wait = Duration::from_std(self.inner.config.post_commit_grace)
            .unwrap_or_else(|_| Duration::zero());
        match *made {
            Some(at) if self.inner.clock.now() - at >= wait => {
                *made = None;
                true
            }
            _ => false,
        }
    }
}

/// Keeps only the threads whose caps were looked at within `wait`: one looked at longer ago is looked at again
/// anyway, so its time says nothing more, and a thread whose turn ended or that was removed leaves the map.
fn forget_stale_checks(
    checked: &mut HashMap<Id, DateTime<Utc>>,
    now: DateTime<Utc>,
    wait: Duration,
) {
    checked.retain(|_, last| now - *last < wait);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_db::usage::caps::split;

    #[test]
    fn the_times_caps_were_looked_at_are_kept_for_one_interval_and_no_longer() {
        let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let wait = Duration::seconds(30);
        let (recent, stale, removed) = (new_id(), new_id(), new_id());
        let mut checked = HashMap::from([
            (recent, now - Duration::seconds(29)),
            (stale, now - Duration::seconds(30)),
            (removed, now - Duration::hours(5)),
        ]);
        forget_stale_checks(&mut checked, now, wait);
        assert_eq!(checked.keys().copied().collect::<Vec<_>>(), [recent]);
    }

    fn cap(subject_kind: &str, split_word: &str, amount: Option<&str>) -> CapRow {
        CapRow {
            id: new_id(),
            node_id: None,
            provider_id: None,
            subject_kind: subject_kind.to_owned(),
            subject_id: None,
            subject_role: None,
            split: split_word.to_owned(),
            models: Vec::new(),
            tiers: Vec::new(),
            harness_kinds: Vec::new(),
            meter: meter::TOKENS.to_owned(),
            period: Some(period::DAY.to_owned()),
            counted_from: None,
            amount: amount.map(str::to_owned),
            at_limit: at_limit::STOP.to_owned(),
            warn_percent: None,
            uncounted: uncounted::ALLOW.to_owned(),
            note: String::new(),
            enabled: true,
            revision: 1,
            created_by: new_id(),
            created_at: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        }
    }

    fn applies(applied: &[Applied], cap: &CapRow) -> bool {
        applied
            .iter()
            .find(|each| each.cap.id == cap.id)
            .is_some_and(|each| each.replaced_by.is_none())
    }

    #[test]
    fn a_cap_that_names_the_person_replaces_the_group_and_role_caps_and_the_one_for_everyone() {
        let mut named = cap(subject::USER, split::SHARED, Some("10"));
        named.subject_id = Some(new_id());
        let mut group = cap(subject::GROUP, split::EACH, Some("5000"));
        group.subject_id = Some(new_id());
        let everyone = cap(subject::EVERYONE, split::EACH, Some("100"));
        let out = precedence(vec![named.clone(), group.clone(), everyone.clone()]);
        assert!(applies(&out, &named));
        assert!(!applies(&out, &group) && !applies(&out, &everyone));
        assert!(
            out.iter()
                .all(|each| each.replaced_by.is_none_or(|by| by == named.id))
        );
    }

    #[test]
    fn of_the_group_and_role_caps_the_largest_applies_and_no_limit_is_the_largest() {
        let mut small = cap(subject::GROUP, split::EACH, Some("1000"));
        small.subject_id = Some(new_id());
        let mut role = cap(subject::ROLE, split::EACH, Some("5000"));
        role.subject_role = Some("developer".to_owned());
        let everyone = cap(subject::EVERYONE, split::EACH, Some("100"));
        let out = precedence(vec![small.clone(), role.clone(), everyone.clone()]);
        assert!(applies(&out, &role));
        assert!(!applies(&out, &small) && !applies(&out, &everyone));

        let mut free = cap(subject::GROUP, split::EACH, None);
        free.subject_id = Some(new_id());
        let out = precedence(vec![small.clone(), role.clone(), free.clone()]);
        assert!(
            applies(&out, &free),
            "a group with no limit exempts its people"
        );
        assert!(!applies(&out, &role));
    }

    #[test]
    fn caps_that_differ_in_what_they_cover_apply_together_and_shared_caps_always_do() {
        let first = cap(subject::EVERYONE, split::EACH, Some("100"));
        let mut other = cap(subject::EVERYONE, split::EACH, Some("200"));
        other.models = vec!["gpt-6*".to_owned()];
        let mut at_a_tier = cap(subject::EVERYONE, split::EACH, Some("250"));
        at_a_tier.tiers = vec!["fast".to_owned()];
        let shared = cap(subject::EVERYONE, split::SHARED, Some("300"));
        let mut also_shared = cap(subject::EVERYONE, split::SHARED, Some("400"));
        also_shared.period = Some(period::DAY.to_owned());
        let out = precedence(vec![
            first.clone(),
            other.clone(),
            at_a_tier.clone(),
            shared.clone(),
            also_shared.clone(),
        ]);
        for each in [&first, &other, &at_a_tier, &shared, &also_shared] {
            assert!(
                applies(&out, each),
                "caps that name other models, or other tiers, are other caps"
            );
        }
    }

    #[test]
    fn caps_that_agree_in_the_tiers_they_name_compete_for_a_person_and_those_that_do_not_do_not() {
        let mut fast_for_all = cap(subject::EVERYONE, split::EACH, Some("100"));
        fast_for_all.tiers = vec!["fast".to_owned()];
        let mut fast_for_one = cap(subject::USER, split::SHARED, Some("10"));
        fast_for_one.tiers = vec!["fast".to_owned()];
        fast_for_one.subject_id = Some(new_id());
        let mut any_for_all = cap(subject::EVERYONE, split::EACH, Some("100"));
        any_for_all.tiers = Vec::new();
        let out = precedence(vec![
            fast_for_all.clone(),
            fast_for_one.clone(),
            any_for_all.clone(),
        ]);
        assert!(applies(&out, &fast_for_one));
        assert!(
            !applies(&out, &fast_for_all),
            "the cap that names the person replaces the one for everyone that names the same tiers"
        );
        assert!(
            applies(&out, &any_for_all),
            "the cap that names no tier is not the one the person's cap replaces"
        );
    }

    #[test]
    fn a_model_pattern_covers_a_family_and_the_same_model_through_a_gateway() {
        let mut capped = cap(subject::EVERYONE, split::SHARED, Some("1"));
        assert!(
            covers_model(&capped, "anything", &[]),
            "no pattern names every model"
        );
        capped.models = vec!["gpt-6*".to_owned()];
        assert!(covers_model(&capped, "gpt-6-sol", &[]));
        assert!(covers_model(&capped, "openai/gpt-6-sol", &[]));
        assert!(!covers_model(&capped, "claude-opus-5", &[]));
        capped.models = vec!["openai/*".to_owned(), "claude-opus-5".to_owned()];
        assert!(covers_model(&capped, "openai/gpt-6-sol", &[]));
        assert!(!covers_model(&capped, "gpt-6-sol", &[]));
        assert!(covers_model(&capped, "anthropic/claude-opus-5", &[]));
    }

    #[test]
    fn a_group_covers_the_models_whose_entries_are_in_it_and_a_tier_the_turns_that_ask_for_it() {
        let in_frontier = ["frontier".to_owned()];
        let mut capped = cap(subject::EVERYONE, split::SHARED, Some("1"));
        capped.models = vec!["@frontier".to_owned(), "claude-*".to_owned()];
        assert!(covers_model(&capped, "gpt-6-sol", &in_frontier));
        assert!(!covers_model(&capped, "gpt-6-sol", &["small".to_owned()]));
        assert!(
            !covers_model(&capped, "gpt-6-sol", &[]),
            "a group no model is in names nothing"
        );
        assert!(
            covers_model(&capped, "claude-opus-5", &[]),
            "a name beside the group"
        );
        assert!(names_a_group(&capped));
        capped.models = vec!["gpt-6*".to_owned()];
        assert!(
            !names_a_group(&capped),
            "nothing is asked of the entries for a cap that names no group"
        );

        // A cap that names no tier covers every turn, the ordinary one too; one that names tiers, those turns.
        let mut at_fast = cap(subject::EVERYONE, split::SHARED, Some("1"));
        assert!(covers_tier(&at_fast, None) && covers_tier(&at_fast, Some("fast")));
        at_fast.tiers = vec!["fast".to_owned(), "flex".to_owned()];
        assert!(covers_tier(&at_fast, Some("fast")) && covers_tier(&at_fast, Some("flex")));
        assert!(!covers_tier(&at_fast, None), "an ordinary turn");
        assert!(!covers_tier(&at_fast, Some("priority")));
    }

    #[test]
    fn the_tier_a_thread_would_ask_for_is_its_choice_where_open_else_what_the_model_starts_with() {
        use orch_agent_contract::parameters::ParameterDef;
        let declared: Vec<ParameterDef> = serde_json::from_value(json!([{
            "key": "service_tier", "title": "Speed", "description": "", "group": "model", "type": "enum",
            "options": { "from_model": "service_tiers" }, "default": { "from_model": "default_service_tier" },
            "scopes": ["thread"], "apply": "next_turn"
        }]))
        .unwrap();
        let model: ModelParameters = serde_json::from_value(json!({
            "service_tiers": [{ "id": "fast", "label": "Fast" }, { "id": "flex", "label": "Flex" }],
            "default_service_tier": "fast"
        }))
        .unwrap();
        let ask = |stored: Json| tier_asked(&declared, &model, &stored);
        assert_eq!(ask(json!({})).as_deref(), Some("fast"), "the model's start");
        assert_eq!(
            ask(json!({ "service_tier": "flex" })).as_deref(),
            Some("flex")
        );
        assert_eq!(
            ask(json!({ "service_tier": "priority" })).as_deref(),
            Some("fast"),
            "a choice that is not open is the start"
        );
        let plain: ModelParameters =
            serde_json::from_value(json!({ "service_tiers": [{ "id": "fast", "label": "Fast" }] }))
                .unwrap();
        assert_eq!(
            tier_asked(&declared, &plain, &json!({})),
            None,
            "the ordinary tier"
        );
        assert_eq!(
            tier_asked(&[], &model, &json!({ "service_tier": "fast" })),
            None
        );
    }

    #[test]
    fn a_period_is_told_in_utc_a_week_from_monday_and_a_month_by_the_calendar() {
        let mut capped = cap(subject::EVERYONE, split::SHARED, Some("1"));
        // Wednesday the 14th of October 2026.
        let now = Utc.with_ymd_and_hms(2026, 10, 14, 13, 30, 0).unwrap();
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let begins = |y, m, d| Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap();
        let w = window(&capped, now);
        assert_eq!(
            (w.start, w.ends_at),
            (Some(day(2026, 10, 14)), Some(begins(2026, 10, 15)))
        );
        capped.period = Some(period::WEEK.to_owned());
        let w = window(&capped, now);
        assert_eq!(
            (w.start, w.ends_at),
            (Some(day(2026, 10, 12)), Some(begins(2026, 10, 19)))
        );
        capped.period = Some(period::MONTH.to_owned());
        let w = window(&capped, now);
        assert_eq!(
            (w.start, w.ends_at),
            (Some(day(2026, 10, 1)), Some(begins(2026, 11, 1)))
        );
        let december = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 0).unwrap();
        assert_eq!(window(&capped, december).ends_at, Some(begins(2027, 1, 1)));
        capped.period = Some(period::TOTAL.to_owned());
        capped.counted_from = Some(day(2026, 9, 1));
        let w = window(&capped, now);
        assert_eq!((w.start, w.ends_at), (Some(day(2026, 9, 1)), None));
        capped.meter = meter::RUNNING.to_owned();
        capped.period = None;
        assert_eq!(
            window(&capped, now),
            Window {
                start: None,
                ends_at: None
            }
        );
    }

    fn standing_of(amount: &str, used: &str, warn: Option<i16>) -> Standing {
        let mut capped = cap(subject::EVERYONE, split::SHARED, Some(amount));
        capped.warn_percent = warn;
        Standing {
            cap: capped,
            person: None,
            amount: Micros::parse(amount),
            used: Micros::parse(used).unwrap(),
            in_progress: Micros::ZERO,
            window: Window {
                start: None,
                ends_at: None,
            },
            unrated_tokens: 0,
        }
    }

    #[test]
    fn a_cap_has_room_while_what_is_used_is_below_its_amount_and_warns_at_its_percent() {
        assert_eq!(standing_of("1000", "799", Some(80)).state(), State::Ok);
        assert_eq!(standing_of("1000", "800", Some(80)).state(), State::Warning);
        assert_eq!(standing_of("1000", "999", Some(80)).state(), State::Warning);
        assert_eq!(
            standing_of("1000", "1000", Some(80)).state(),
            State::Reached
        );
        assert_eq!(
            standing_of("0", "0", None).state(),
            State::Reached,
            "a cap of nothing is full"
        );
        let mut with_progress = standing_of("1000", "900", None);
        with_progress.in_progress = Micros::whole(100);
        assert!(!with_progress.has_room(), "what runs turns counts");
        assert_eq!(
            standing_of("1000", "400", None).room_left(),
            Some(Micros::whole(600))
        );
        assert_eq!(
            standing_of("1000", "1400", None).room_left(),
            Some(Micros::ZERO)
        );
    }

    #[test]
    fn the_tightest_cap_that_would_stop_a_turn_decides_what_its_lease_may_use() {
        let tokens = |amount: &str, used: &str, at| {
            let mut each = standing_of(amount, used, None);
            each.cap.at_limit = at;
            each
        };
        let mut credits = standing_of("12.5", "2.5", None);
        credits.cap.meter = meter::CREDITS.to_owned();
        let standings = vec![
            tokens("1000", "100", at_limit::STOP.to_owned()),
            tokens("500", "100", at_limit::STOP.to_owned()),
            tokens("50", "10", at_limit::FINISH_TURN.to_owned()),
            credits,
        ];
        let room = room_left(&standings, &[]);
        assert_eq!(
            room.tokens,
            Some(400),
            "a cap that lets a turn finish is not in the way of one that runs"
        );
        assert_eq!(room.credits, Some(Micros::whole(10)));
        let passed = vec![Passed {
            standing: standings[1].clone(),
            by: new_id(),
        }];
        assert_eq!(
            room_left(&standings, &passed).tokens,
            Some(900),
            "a cap the turn was let past is left out"
        );
    }

    #[test]
    fn what_stands_in_the_way_is_told_with_its_numbers_grouped() {
        assert_eq!(shown(Micros::whole(2_000_000)), "2 000 000");
        assert_eq!(shown(Micros::parse("12.5").unwrap()), "12.5");
        assert_eq!(shown(Micros::whole(7)), "7");
        assert_eq!(capitalised("project"), "Project");
    }

    #[test]
    fn bypass_entries_are_read_and_written_in_the_form_the_input_stores() {
        let (by, cap_id) = (new_id(), new_id());
        let stored = json!([{ "by": by, "at": "2026-10-01T00:00:00.000Z", "caps": [cap_id], "provider_limit": false }]);
        let entries = entries_of(Some(&stored));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].caps, vec![cap_id]);
        assert!(bypass_names(Some(&stored), cap_id));
        assert!(!bypass_names(Some(&stored), new_id()));
        assert_eq!(entries_json(&entries), Some(stored));
        let empty = [Entry {
            caps: Vec::new(),
            provider_limit: false,
            ..entries[0].clone()
        }];
        assert_eq!(
            entries_json(&empty),
            None,
            "an entry that passes nothing is not kept"
        );
    }
}
