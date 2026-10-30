//! Orgs inside orgs as the homes of resources: whose defaults and admins a resource has, and its address; who administers a place; a member's domain, and membership of every org from it upward; the depth the system allows; a person's own org and the two limits of a root org.
//!
//! This part holds what is known of a person's membership: the trees of
//! orgs they belong to, with their domain in each, and so every org they
//! are a member of (their domain and each org above it). It also holds the
//! two limits a root org has and the arithmetic of depth. Resources, their
//! homes and addresses are a later part.
//!
//! Holding a role in an org from above makes nobody a member of it, and a
//! system admin is a member of none by being one: membership is the
//! domain's path and nothing else.

use crate::Error;
use orch_db::Conn;
use orch_db::queries::{access, hierarchy};
use orch_types::Id;

/// A person's membership of one tree of orgs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    /// The root org of the tree.
    pub root: Id,
    /// Their domain: the root org itself or an org inside it.
    pub domain: Id,
    /// The orgs from the root org down to the domain, both included.
    pub path: Vec<Id>,
}

/// Every tree of orgs a person belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memberships {
    trees: Vec<Membership>,
}

impl Memberships {
    /// The memberships of `user`.
    pub async fn load(conn: &mut Conn, user: Id) -> Result<Self, Error> {
        let rows = access::memberships(conn, user).await?;
        Ok(Self {
            trees: rows
                .into_iter()
                .map(|row| Membership {
                    root: row.org_id,
                    domain: row.domain_id,
                    path: row.domain_path,
                })
                .collect(),
        })
    }

    /// Memberships as they are given.
    #[must_use]
    pub fn of(trees: Vec<Membership>) -> Self {
        Self { trees }
    }

    /// Each tree, with the domain in it.
    #[must_use]
    pub fn trees(&self) -> &[Membership] {
        &self.trees
    }

    /// Every org they are a member of: in each tree, their domain and the
    /// orgs above it up to the root org. Sorted, each once.
    #[must_use]
    pub fn member_of(&self) -> Vec<Id> {
        let mut orgs: Vec<Id> = self
            .trees
            .iter()
            .flat_map(|tree| tree.path.iter().copied())
            .collect();
        orgs.sort_unstable();
        orgs.dedup();
        orgs
    }

    /// The root orgs they belong to, sorted.
    #[must_use]
    pub fn roots(&self) -> Vec<Id> {
        let mut roots: Vec<Id> = self.trees.iter().map(|tree| tree.root).collect();
        roots.sort_unstable();
        roots
    }

    /// Whether they belong to the tree of the root org `root`.
    #[must_use]
    pub fn is_in_tree(&self, root: Id) -> bool {
        self.trees.iter().any(|tree| tree.root == root)
    }

    /// Their domain in the tree of `root`.
    #[must_use]
    pub fn domain_in(&self, root: Id) -> Option<Id> {
        self.trees
            .iter()
            .find(|tree| tree.root == root)
            .map(|tree| tree.domain)
    }

    /// Whether they are a member of `org`, at any level.
    #[must_use]
    pub fn is_member_of(&self, org: Id) -> bool {
        self.trees
            .iter()
            .any(|tree| orch_access_contract::roles::is_member_of(&tree.path, org))
    }
}

/// The two limits of a root org, set from above it by the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Its tree takes no member beyond those it has.
    pub closed: bool,
    /// No org is made inside it, and none moved into it.
    pub flat: bool,
}

/// The limits of the root org `root`, or nothing where there is no such
/// org.
pub async fn limits(conn: &mut Conn, root: Id) -> Result<Option<Limits>, Error> {
    Ok(hierarchy::org(conn, root).await?.map(|org| Limits {
        closed: org.closed,
        flat: org.flat,
    }))
}

/// Whether a path of `orgs` orgs one inside another is within the depth the
/// system allows, `limit` (`[limits] org_depth`).
#[must_use]
pub fn fits_depth(orgs: usize, limit: u32) -> bool {
    u32::try_from(orgs).is_ok_and(|orgs| orgs <= limit)
}

/// Whether a path of `projects` projects one inside another is within the depth the system
/// allows, `limit` (`[limits] project_depth`): a project in an org is the first of them, and
/// each folder in a project one more.
#[must_use]
pub fn fits_project_depth(projects: i64, limit: u32) -> bool {
    u32::try_from(projects).is_ok_and(|projects| projects <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn a_member_is_a_member_of_their_domain_and_every_org_above_it_and_of_none_beside() {
        let (acme, research, ml, sales) = (new_id(), new_id(), new_id(), new_id());
        let memberships = Memberships::of(vec![Membership {
            root: acme,
            domain: ml,
            path: vec![acme, research, ml],
        }]);
        for org in [acme, research, ml] {
            assert!(memberships.is_member_of(org));
        }
        assert!(!memberships.is_member_of(sales));
        assert_eq!(memberships.member_of().len(), 3);
        assert_eq!(memberships.domain_in(acme), Some(ml));
        assert_eq!(memberships.domain_in(sales), None);
        assert!(memberships.is_in_tree(acme));
        assert_eq!(memberships.roots(), vec![acme]);
    }

    #[test]
    fn two_trees_make_two_domains_and_the_orgs_of_both() {
        let (acme, globex, globex_ops) = (new_id(), new_id(), new_id());
        let memberships = Memberships::of(vec![
            Membership {
                root: acme,
                domain: acme,
                path: vec![acme],
            },
            Membership {
                root: globex,
                domain: globex_ops,
                path: vec![globex, globex_ops],
            },
        ]);
        let mut expected = vec![acme, globex, globex_ops];
        expected.sort_unstable();
        assert_eq!(memberships.member_of(), expected);
        assert_eq!(memberships.domain_in(globex), Some(globex_ops));
    }

    #[test]
    fn a_path_may_hold_as_many_orgs_as_the_limit_says_and_no_more() {
        assert!(fits_depth(8, 8));
        assert!(!fits_depth(9, 8));
        assert!(fits_depth(1, 8));
    }
}
