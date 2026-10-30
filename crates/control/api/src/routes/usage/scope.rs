//! What a reader of usage is shown, by the scope they ask about: their own use, a place and everything below it, or what ran on a provider; how deep their access goes there, and which dimensions and filters that depth gives.
//!
//! The access engine decides the depth ([`orch_authz::caps::depth_at`]); this
//! module only turns the decision into what a request may ask. A dimension or a
//! filter the depth does not give is refused with `403 authz.denied`, naming
//! `usage.view`: a thread at the depth of a summary, and a person beyond oneself
//! at the depth of one's own use.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use chrono::{Datelike, NaiveDate};
use http::{HeaderMap, header};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::usage::{UsageDepth, UsageDimension, UsageEnvironmentKind};
use orch_authz::caps::{Depth, Home, Verdict, check_view, depth_at};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::usage::rollup::{Filter, Place, place_of};
use orch_types::Id;

/// What a request asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// What the caller used themself, anywhere.
    Me,
    /// A place and everything below it.
    Node(Id),
    /// What ran on a provider, of whatever org.
    Provider(Id),
}

/// Reads the `scope` of a request: `me`, `node:` and an identifier, or `provider:` and an identifier.
pub fn parse(text: &str) -> ApiResult<Scope> {
    let bad = || {
        ApiError::invalid(
            "scope",
            "is `me`, `node:` and an identifier, or `provider:` and an identifier",
        )
    };
    if text == "me" {
        return Ok(Scope::Me);
    }
    if let Some(id) = text.strip_prefix("node:") {
        return id.parse().map(Scope::Node).map_err(|_| bad());
    }
    if let Some(id) = text.strip_prefix("provider:") {
        return id.parse().map(Scope::Provider).map_err(|_| bad());
    }
    Err(bad())
}

/// The API's four-digit calendar dates, all representable by the database. Chrono also
/// accepts signed six-digit years outside PostgreSQL's timestamp range.
pub(crate) fn date_in_range(day: NaiveDate) -> bool {
    (1..=9999).contains(&day.year())
}

/// A day as `YYYY-MM-DD`, UTC, in the API's four-digit calendar.
pub fn parse_day(path: &str, text: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .filter(|day| date_in_range(*day))
        .ok_or_else(|| ApiError::invalid(path, "is a day, written YYYY-MM-DD"))
}

/// Whether the reader asked for CSV rather than JSON.
#[must_use]
pub fn wants_csv(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| {
            accept
                .split(',')
                .any(|part| part.trim().starts_with("text/csv"))
        })
}

/// A scope the caller was let into, and how deep.
#[derive(Debug, Clone)]
pub struct Opened {
    /// What was asked about, as the answer names it.
    pub scope: String,
    /// How deep the caller is shown.
    pub depth: UsageDepth,
    /// What the scope itself limits the rows to.
    pub base: Filter,
    /// Who asks.
    pub caller: Id,
    /// The place asked about, where it is one.
    pub node: Option<Id>,
    /// Whether the answer carries the caller's own part (for a place or a provider).
    pub has_own_part: bool,
}

/// What a request limits the rows to beyond the scope.
#[derive(Debug, Clone, Default)]
pub struct Wanted {
    /// Only the use of this person.
    pub user_id: Option<Id>,
    /// Only this model.
    pub model: Option<String>,
    /// Only what ran on this provider.
    pub provider_id: Option<Id>,
    /// Only this harness kind.
    pub harness_kind: Option<String>,
    /// Only this kind of environment.
    pub environment_kind: Option<UsageEnvironmentKind>,
    /// Only this project.
    pub project_id: Option<Id>,
    /// Only this topic.
    pub topic_id: Option<Id>,
    /// Only this thread.
    pub thread_id: Option<Id>,
}

/// The refusal for what the reader's depth does not give: `403 authz.denied`, naming `usage.view`.
async fn deny(conn: &mut Conn, principal: &Principal, opened: &Opened, what: &str) -> ApiError {
    if let Some(node) = opened.node
        && let Ok(orch_access_contract::decisions::Decision::Refused(refusal)) =
            orch_authz::nodes::check(
                conn,
                principal,
                node,
                orch_access_contract::permissions::Permission::UsageView,
                orch_authz::Act::Use,
                Policy::default(),
            )
            .await
        && let Ok(error) = refusals::denied(conn, refusal).await
    {
        return error;
    }
    ApiError::new(
        ErrorCode::AuthzDenied,
        format!("{what} needs usage.view here"),
    )
    .with("required", "usage.view")
    .with(
        "object",
        serde_json::json!({ "kind": opened.node.map_or("user", |_| "node"), "id": opened.node.unwrap_or(opened.caller) }),
    )
    .with("requestable", false)
    .with("override", false)
    .with("missing", Vec::<serde_json::Value>::new())
}

/// Lets the caller into the scope they ask about, and says how deep they are shown there.
pub async fn open(conn: &mut Conn, principal: &Principal, scope: Scope) -> ApiResult<Opened> {
    let caller = principal.user();
    match scope {
        Scope::Me => Ok(Opened {
            scope: "me".to_owned(),
            depth: UsageDepth::Own,
            base: Filter {
                person: Some(caller),
                ..Filter::default()
            },
            caller,
            node: None,
            has_own_part: false,
        }),
        Scope::Node(node) => {
            let place = place_of(conn, node)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let depth = match depth_at(conn, principal, node, Policy::default()).await {
                Ok(Some(depth)) => depth,
                Ok(None) => {
                    // Not even sight of the place: refused as for any place.
                    authorization::require(
                        conn,
                        principal,
                        node,
                        authorization::VIEW,
                        orch_authz::Act::Use,
                        Policy::default(),
                    )
                    .await?;
                    return Err(ApiError::not_found());
                }
                Err(error) => return Err(refusals::problem(conn, error).await),
            };
            let own = depth == Depth::Own;
            Ok(Opened {
                scope: format!("node:{node}"),
                depth: match depth {
                    Depth::Own => UsageDepth::Own,
                    Depth::Summary => UsageDepth::Summary,
                    Depth::Reports => UsageDepth::Reports,
                },
                base: Filter {
                    node: Some(Place { id: node, ..place }),
                    person: own.then_some(caller),
                    ..Filter::default()
                },
                caller,
                node: Some(node),
                has_own_part: true,
            })
        }
        Scope::Provider(provider) => {
            match check_view(
                conn,
                principal,
                Home::Provider {
                    provider,
                    node: None,
                },
                Policy::default(),
            )
            .await
            {
                Ok(Verdict::Allowed) => {}
                Ok(Verdict::Refused(refusal)) => {
                    return Err(refusals::denied(conn, *refusal).await?);
                }
                Ok(Verdict::SystemAdminOnly) => return Err(ApiError::not_found()),
                Err(error) => return Err(refusals::problem(conn, error).await),
            }
            Ok(Opened {
                scope: format!("provider:{provider}"),
                depth: UsageDepth::Reports,
                base: Filter {
                    provider: Some(provider),
                    ..Filter::default()
                },
                caller,
                node: None,
                has_own_part: true,
            })
        }
    }
}

/// Whether the depth gives a dimension at all.
#[must_use]
pub fn gives(depth: UsageDepth, dimension: UsageDimension) -> bool {
    !(depth == UsageDepth::Summary && dimension == UsageDimension::Thread)
}

/// Refuses a dimension the reader's depth does not give.
pub async fn require_dimension(
    conn: &mut Conn,
    principal: &Principal,
    opened: &Opened,
    dimension: UsageDimension,
    what: &str,
) -> ApiResult<()> {
    if gives(opened.depth, dimension) {
        Ok(())
    } else {
        Err(deny(conn, principal, opened, what).await)
    }
}

/// The filter of a request: the scope's own limits and what the request adds, with what the depth refuses.
/// `records` says the request is for records, which a summary gives only of the caller's own.
pub async fn filter(
    conn: &mut Conn,
    principal: &Principal,
    opened: &Opened,
    wanted: &Wanted,
    records: bool,
) -> ApiResult<Filter> {
    let mut filter = opened.base.clone();
    if !records && wanted.thread_id.is_some() && opened.depth == UsageDepth::Summary {
        return Err(deny(conn, principal, opened, "the use of one thread").await);
    }
    if let Some(user) = wanted.user_id {
        let beyond_oneself = user != opened.caller;
        if beyond_oneself && opened.depth == UsageDepth::Own {
            return Err(deny(conn, principal, opened, "somebody else's use").await);
        }
        if beyond_oneself && records && opened.depth == UsageDepth::Summary {
            return Err(deny(conn, principal, opened, "somebody else's records").await);
        }
        filter.person = Some(user);
    }
    if records && opened.depth == UsageDepth::Summary {
        // A summary gives no record but the caller's own.
        filter.person = Some(opened.caller);
    }
    if let Some(provider) = wanted.provider_id {
        if filter.provider.is_some_and(|scoped| scoped != provider) {
            return Err(ApiError::invalid(
                "provider_id",
                "is not the provider the scope names",
            ));
        }
        filter.provider = Some(provider);
    }
    filter.model.clone_from(&wanted.model);
    filter.harness_kind.clone_from(&wanted.harness_kind);
    filter.environment_kind = wanted.environment_kind.map(|kind| match kind {
        UsageEnvironmentKind::Managed => "managed".to_owned(),
        UsageEnvironmentKind::Connected => "connected".to_owned(),
    });
    filter.project = wanted.project_id;
    filter.topic = wanted.topic_id;
    filter.thread = wanted.thread_id;
    Ok(filter)
}

/// Provider totals include all places. Only provider managers may list rows from hidden places.
pub async fn all_record_places(
    conn: &mut Conn,
    principal: &Principal,
    opened: &Opened,
) -> ApiResult<bool> {
    let Some(provider) = opened.base.provider else {
        return Ok(true);
    };
    orch_authz::caps::check_manage(
        conn,
        principal,
        Home::Provider {
            provider,
            node: None,
        },
        Policy::default(),
    )
    .await
    .map(|verdict| verdict.is_allowed())
    .map_err(ApiError::internal)
}

/// Applies the canonical node-view rule to every candidate before its id/time can leave the server.
pub async fn visible_records(
    conn: &mut Conn,
    principal: &Principal,
    all_places: bool,
    rows: Vec<orch_db::usage::ledger::EventRow>,
) -> ApiResult<Vec<orch_db::usage::ledger::EventRow>> {
    if all_places || rows.is_empty() {
        return Ok(rows);
    }
    let candidates: Vec<Id> = rows
        .iter()
        .map(|row| row.thread_id.unwrap_or(row.topic_id))
        .collect();
    let visible: std::collections::HashSet<Id> = orch_authz::nodes::holding(
        conn,
        principal,
        &candidates,
        authorization::VIEW,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|listed| listed.id)
    .collect();
    Ok(rows
        .into_iter()
        .filter(|row| visible.contains(&row.thread_id.unwrap_or(row.topic_id)))
        .collect())
}
