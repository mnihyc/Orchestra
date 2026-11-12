//! The ledger rows behind a figure, with the same scope and filters, each with its thread and its turn, what it ran on, the rate it was counted at and its credits; a page of them, or all of a page as CSV.
//!
//! A row is shown whole to a reader who may see everybody's records in the
//! scope, and only their own to one who sees a place's figures, so that a figure
//! can be followed row by row as far as the reader's access goes. A thread the
//! reader cannot see is a reference without a name, and nothing of its
//! conversation is shown.

pub use super::scope;

pub use crate::middleware::requests::ApiResult;

pub use orch_db::usage::ledger;

pub use orch_db::usage::ledger::EventRow;

pub use orch_db::usage::ledger::Listing;
/// Page in bounded candidate batches until the visible page is full. A hidden row never becomes
/// a public cursor; neither an id nor its time is exposed to a provider's ordinary users.
pub async fn record_page(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    all_places: bool,
    listing: &Listing<'_>,
    include_running: bool,
    requests_only: bool,
    through: Option<i64>,
) -> ApiResult<Vec<EventRow>> {
    let mut candidate = listing.clone();
    let mut visible = Vec::new();
    loop {
        let rows = if let Some(through) = through {
            ledger::live_snapshot(conn, &candidate, include_running, through).await?
        } else {
            ledger::list_live(conn, &candidate, include_running, requests_only).await?
        };
        let full = rows.len() == usize::try_from(candidate.limit).unwrap_or(usize::MAX);
        candidate.after = rows.last().map(|row| (row.at, row.id));
        visible.extend(scope::visible_records(conn, principal, all_places, rows).await?);
        if visible.len() >= usize::try_from(listing.limit).unwrap_or(usize::MAX) || !full {
            break;
        }
    }
    visible.truncate(usize::try_from(listing.limit).unwrap_or(usize::MAX));
    Ok(visible)
}

orch_api_base::api_family_callback! {
    bind __api_bind_87631274e1, __api_install_87631274e1, __api_callback_87631274e1;
    slot __ApiCallback87631274e1, __API_CALLBACK_87631274E1, __api_set_87631274e1;
    base orch_api_base::routes::usage::events;

    async fn events<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_db::usage::ledger::EventRow]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::usage::UsageEvent>>;
}
