//! Everything a caller lacks for an action, the nearest people who can give each part and how many more could, whether the caller may force it as an admin of the place, and what nobody can give and why: for refusals, the watching card and access requests.
//!
//! A refusal names everything the caller lacks, object by object, and one
//! request can then ask for all of it. For each part it says who can give
//! it: the people who would pass the check for changing the page at this
//! moment, the nearest first. It says whether the caller may force the act
//! themself, because they administer the place and a page left them out;
//! and where nobody can give a part, because a lock above decides the row,
//! it says so and where the lock is.
//!
//! Who can give a permission depends on the permission. Changing a page is
//! for its owners, whoever its page names in `access.manage`, and the
//! admins of the place. Naming holders of `access.manage` and changing the
//! Owners row is for the owners and the admins. Naming holders of
//! `access.external` is for the admins alone.

use orch_access_contract::decisions::{
    Giver, MissingAccess, MissingObject, Nearness, ObjectRef, Refusal,
};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{NamedBy, Permission};
use orch_access_contract::roles::PlaceRef;
use orch_db::Conn;
use orch_db::queries::access;
use orch_types::Id;

use crate::Error;
use crate::engine::{Frame, Site};
use crate::holders::{admins_at, owner_people, people_of};

use crate::holders::SCAN_MOST;

/// The place whose lock closes `permission` on the site to everybody: its
/// row is decided by a lock above. A request for it adds an entry to that
/// row, which a lock refuses, and so does a override: nobody gives it on this
/// page, and the way on is whoever may change the defaults where the lock is.
#[must_use]
pub fn closed_by_lock(frame: &Frame<'_>, site: &Site, permission: Permission) -> Option<PlaceRef> {
    frame.chain(site, permission).locked_by()
}

/// The refusal of an act that needs `required` on the site, for which the
/// caller lacks `lacking`. The frame has read every permission of the
/// site's kind.
pub async fn refusal(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    required: Permission,
    lacking: &[Permission],
) -> Result<Refusal, Error> {
    let known = frame.holds_any(site);
    let missing = missing_access(conn, frame, site, lacking, known).await?;
    Ok(Refusal::of(
        required,
        ObjectRef {
            kind: site.kind,
            id: site.id,
        },
        missing,
    ))
}

/// What the caller lacks on the one site, as the entries of a refusal: the permissions in
/// `lacking`, grouped by who can give them and by the lock above that closes them, each with the
/// nearest people who can give it. The object is named, and its first owner given, when `known`:
/// the caller holds something on it, or can see the thing that brought them to it. The frame has
/// read every permission of the site's kind.
///
/// A refusal that is about several things is assembled from this, one site after another.
pub async fn missing_access(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    lacking: &[Permission],
    known: bool,
) -> Result<Vec<MissingAccess>, Error> {
    let object = MissingObject {
        kind: site.kind,
        id: site.id,
        name: known.then(|| site.name.clone()),
        owner: if known { site.first_owner } else { None },
    };
    let administers =
        frame.administers(site) && frame.principal().caller().permits(Permission::AccessManage);

    struct Bucket {
        lock: Option<PlaceRef>,
        by: NamedBy,
        who: Who,
        permissions: Vec<Permission>,
    }
    let mut buckets: Vec<Bucket> = Vec::new();
    for permission in lacking {
        let lock = closed_by_lock(frame, site, *permission);
        let by = permission.named_by();
        match buckets
            .iter_mut()
            .find(|bucket| bucket.lock == lock && bucket.by == by)
        {
            Some(bucket) => bucket.permissions.push(*permission),
            None => buckets.push(Bucket {
                lock,
                by,
                who: Who::to_give(*permission),
                permissions: vec![*permission],
            }),
        }
    }
    let mut missing = Vec::with_capacity(buckets.len());
    for bucket in buckets {
        missing.push(match bucket.lock {
            Some(place) => MissingAccess::locked(object.clone(), bucket.permissions, place),
            None => {
                let givers = givers(conn, frame, site, bucket.who).await?;
                MissingAccess::open(
                    object.clone(),
                    bucket.permissions,
                    None,
                    &givers,
                    administers,
                )
            }
        });
    }
    Ok(missing)
}

/// Which kinds of people may do something to a page: its owners, the
/// holders of a row, and the admins of the places above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Who {
    /// Its owners.
    pub owners: bool,
    /// Whoever the page names in this row.
    pub named_in: Option<Permission>,
    /// The admins of the places above it.
    pub admins: bool,
}

impl Who {
    /// Who can give `permission` on a page: for most permissions whoever
    /// can change the page; for `access.manage` and the Owners row its
    /// owners and admins; for `access.external` the admins alone.
    #[must_use]
    pub fn to_give(permission: Permission) -> Self {
        match permission.named_by() {
            NamedBy::WhoeverChangesThePage => Self {
                owners: true,
                named_in: Some(Permission::AccessManage),
                admins: true,
            },
            NamedBy::OwnerOrAdmin => Self {
                owners: true,
                named_in: None,
                admins: true,
            },
            NamedBy::Admin => Self {
                owners: false,
                named_in: None,
                admins: true,
            },
        }
    }

    /// Who may change a page: its owners, whoever it names in
    /// `access.manage`, and the admins of the places above it.
    #[must_use]
    pub fn to_change_the_page() -> Self {
        Self {
            owners: true,
            named_in: Some(Permission::AccessManage),
            admins: true,
        }
    }
}

/// The people who could do what `who` says to the site now: its owners,
/// whoever its page names in the row, and the admins of the places above
/// it, each with how near they stand. Only people who are active where the
/// site is are listed.
pub async fn givers(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    who: Who,
) -> Result<Vec<Giver>, Error> {
    let mut found: Vec<Giver> = Vec::new();
    if who.owners {
        for user in owner_people(conn, frame, site.id, site.root).await? {
            found.push(Giver {
                user,
                nearness: Nearness::Owner,
            });
        }
    }
    if let Some(row) = who.named_in {
        let named: Vec<Holder> = frame
            .chain(site, row)
            .holders(frame.now())
            .into_iter()
            .map(|named| named.entry.holder)
            .collect();
        for holder in named {
            for user in people_of(conn, frame, site, &holder).await? {
                found.push(Giver {
                    user,
                    nearness: Nearness::Named,
                });
            }
        }
    }
    if who.admins {
        for (above, user) in admins_at(conn, frame, &site.admin_places).await? {
            found.push(Giver {
                user,
                nearness: Nearness::Admin {
                    above: u32::try_from(above).unwrap_or(u32::MAX),
                },
            });
        }
        let system_distance = u32::try_from(site.admin_places.len()).unwrap_or(u32::MAX);
        for user in access::system_admin_people(conn, SCAN_MOST).await? {
            found.push(Giver {
                user,
                nearness: Nearness::Admin {
                    above: system_distance,
                },
            });
        }
    }
    let mut users: Vec<Id> = found.iter().map(|giver| giver.user).collect();
    users.sort_unstable();
    users.dedup();
    let active = access::active_among(conn, &users, &site.orgs).await?;
    found.retain(|giver| active.contains(&giver.user));
    Ok(found)
}

/// The people who may name somebody from outside the root org on the
/// site's page now: those who may change the page and hold the right to name
/// an outsider on it, which is whoever its page names in `access.external`
/// and, where the policy lets them, its owners. Nearest first.
pub async fn who_may_name_outsiders(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
) -> Result<Vec<Giver>, Error> {
    let changers = givers(conn, frame, site, Who::to_change_the_page()).await?;
    let mut entitled: Vec<Id> = Vec::new();
    let named: Vec<Holder> = frame
        .chain(site, Permission::AccessExternal)
        .holders(frame.now())
        .into_iter()
        .map(|named| named.entry.holder)
        .collect();
    for holder in named {
        entitled.extend(people_of(conn, frame, site, &holder).await?);
    }
    if frame.policy().owners_name_outsiders {
        entitled.extend(owner_people(conn, frame, site.id, site.root).await?);
    }
    Ok(changers
        .into_iter()
        .filter(|giver| entitled.contains(&giver.user))
        .collect())
}
