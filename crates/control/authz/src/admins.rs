//! Explicit session administration, its native View and Manage reach, remaining advanced overrides, and what stays closed to everybody.
//!
//! An admin of a place is whoever holds the Admin role there or in a place
//! above it, its owners, and the system's admins in a browser session whose
//! As admin mode is enabled. They hold View and Manage, and what those levels
//! imply, on what they administer. Their ordinary pages are compared beside
//! that reach so a reader knows when an object or act depends on it. Tokens,
//! agents, subscriptions and notification audiences never inherit that mode.
//!
//! Viewing and managing require no self-naming override. An advanced act
//! outside those native levels can still use the ordinary audited override
//! where allowed. A setting or policy that closes an act to everybody stays
//! closed, including for an administrator.

use orch_access_contract::decisions::{self, Through};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_types::Id;

use crate::engine::{Frame, Site};
use crate::missing::closed_by_lock;

/// Whether the person administers the site: they hold Admin at the place it
/// lies in or at a place above it, are an owner of such a place, or are a
/// system admin, with this browser session's As admin mode enabled.
#[must_use]
pub fn administers(frame: &Frame<'_>, site: &Site) -> bool {
    frame.administers(site)
}

/// Whether the person is an Admin at the place `place` itself: given the
/// role there or above it, an owner of it or of a place above, or a system
/// admin. It is what giving and taking away Admin there takes.
#[must_use]
pub fn is_admin_at(frame: &Frame<'_>, place: Id) -> bool {
    frame
        .role_at(place)
        .is_some_and(|held| held.role == Role::Admin)
}

/// Where the lock is that closes `permission` on the site to everybody, an
/// admin among them, if there is one.
#[must_use]
pub fn closed_to_everybody(
    frame: &Frame<'_>,
    site: &Site,
    permission: Permission,
) -> Option<PlaceRef> {
    closed_by_lock(frame, site, permission)
}

/// Whether the person may force an act that needs `permission` on the site:
/// they administer it, they do not hold the permission, and nothing closes
/// it to everybody.
#[must_use]
pub fn may_force(frame: &Frame<'_>, site: &Site, permission: Permission) -> bool {
    frame.administers(site)
        && !frame.ways(site, Permission::AccessManage).is_empty()
        && frame.principal().caller().permits(permission)
        && frame.ways(site, permission).is_empty()
        && closed_to_everybody(frame, site, permission).is_none()
}

/// Whether naming the person in a row they did not hold is an override, to be
/// marked so: they hold the right to change the page by administering it
/// and by nothing else (`manages`), and held nothing in that row (`held`).
#[must_use]
pub fn is_override(manages: &[Through], held: &[Through]) -> bool {
    decisions::is_override(manages, held)
}
