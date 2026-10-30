//! Management: users and their suspension, a reset link that may switch a second step off, what a new person's own org starts as, root orgs and their two limits, both pools of runtimes and their updates, stored metrics, audit and system state.
//!
//! Everything here is the system's own. The binder lets in a browser session only; each handler
//! then asks the one question the binder cannot, whether that person is a system admin, before it
//! reads or changes anything, so that an admin of an org, who is a system admin of nothing, is
//! refused like anybody else. The acts that change a person's standing are the access engine's
//! (`orch_authz::standing`), the same functions the operator's command line calls, here with the
//! signed-in admin as the actor of the audit line; the others write their line in the transaction
//! that makes the change, so that neither exists without the other.
//!
//! The parts are:
//!
//! - [`users`] lists and makes people, suspends and lets them in again, makes and unmakes system
//!   admins, and makes the link with which somebody sets a password anew.
//! - [`orgs`] lists every org and sets the two limits of a root org.
//! - [`semantic`] says where search by meaning stands for each root org, rebuilds an org's index in place
//!   and makes the provider Local embeddings.
//! - [`settings`] and [`defaults`] are the system's settings and its access defaults, which are
//!   the settings engine's and the access engine's with the system as the place.
//! - [`audit`] reads the audit trail of the whole system and writes a span of it as a file.
//! - [`system`] says how the system stands: its parts, its queues, its blob store, its
//!   certificates, its OpenAPI document and the certificate of its authority.
//! - [`fleet`] lists the hosts, the runtimes, the stored metrics and the plugins.
//! - [`catalogue`] is the data admins keep: capability definitions, harness builds and the
//!   agent profiles of the system.
//! - [`identity_providers`] keeps the single sign-in methods.

pub mod audit;
pub mod catalogue;
pub mod defaults;
pub mod fleet;
pub mod identity_providers;
pub mod orgs;
pub mod programs;
pub mod semantic;
pub mod settings;
pub mod system;
pub mod users;

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::sign_in;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::common::PageQuery;
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_types::{Id, Json};

/// Seconds in a day, to keep a span of days from overflowing.
pub(crate) const SECONDS_PER_DAY: i64 = 86_400;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    users::mount(registry);
    orgs::mount(registry);
    semantic::mount(registry);
    settings::mount(registry);
    defaults::mount(registry);
    audit::mount(registry);
    system::mount(registry);
    fleet::mount(registry);
    catalogue::mount(registry);
    programs::mount(registry);
    identity_providers::mount(registry);
}

/// The signed-in system admin of this request. Anybody else is refused, whatever else they hold:
/// an org's Admin is an Admin of that org, and of nothing that is the system's.
pub(crate) fn admin_of(ctx: &Ctx) -> ApiResult<Id> {
    let authed = ctx.user()?;
    if authed.is_system_admin() {
        Ok(authed.id())
    } else {
        Err(refusals::system_admin_only(
            "only a system admin does this, in a browser session",
        ))
    }
}

/// How many entries a list returns, by the limit it was asked for.
pub(crate) fn page_size(limit: Option<u32>) -> usize {
    usize::try_from(
        PageQuery {
            limit,
            cursor: None,
        }
        .limit(),
    )
    .unwrap_or(usize::MAX)
}

/// One more than a page, to see whether another page follows.
pub(crate) fn read_ahead(size: usize) -> i64 {
    i64::try_from(size.saturating_add(1)).unwrap_or(i64::MAX)
}

/// Writes the audit line of an act of the signed-in admin that is about no node, in the caller's
/// transaction. `object` is the thing it is about, where it is about one.
pub(crate) async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    admin: Id,
    action: AuditAction,
    object: Option<Id>,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    sign_in::record(
        ctx,
        conn,
        Actor::User(admin),
        action,
        object,
        summary,
        details,
    )
    .await
}

/// A fault of one member of a request.
pub(crate) fn invalid(path: &str, message: &str) -> ApiError {
    crate::routes::nodes::invalid(path, message)
}

/// Whether `url` is an address that a program can fetch from: `https`, or `http` where something
/// else, such as a hash, says whether to trust what comes. It names a host and holds no space.
pub(crate) fn is_address(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.chars().count() <= URL_MAX_CHARS
        && !url
            .chars()
            .any(|each| each.is_whitespace() || each.is_control())
        && rest.is_some_and(|rest| rest.split('/').next().is_some_and(|host| !host.is_empty()))
}

/// The most characters of an address that is kept.
const URL_MAX_CHARS: usize = 2_048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_http_or_https_with_a_host_and_no_space() {
        assert!(is_address("https://example.com/codex"));
        assert!(is_address("http://127.0.0.1:8080/x"));
        for bad in [
            "ftp://example.com/x",
            "https://",
            "https:///x",
            "https://a b/x",
            "",
        ] {
            assert!(!is_address(bad), "{bad}");
        }
    }
}
