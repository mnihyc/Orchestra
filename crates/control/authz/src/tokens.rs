//! API-token scope and subtree narrowing; nothing that its user holds as a system admin.
//!
//! A request is made by a [`Caller`]: a person in a browser session, who
//! holds whatever their roles and pages give them and, as a system admin,
//! Admin everywhere; or an API token, which acts as its user within the
//! [scopes](TokenGrant::carries) it was made with, inside the subtree it
//! is kept to, and without anything its user holds as a system admin. A
//! token that leaks is so never the whole system.
//!
//! Narrowing only ever takes away: it is applied to what the user holds,
//! never to add to it.

use orch_access_contract::decisions::{Scopes, token_reaches};
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{Credential, UnknownName};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};

mod captured;
mod captured_operation;
mod operation_scope;

pub use captured::{CapturedCaller, with_owner_read_source};
pub use captured_operation::CapturedOperation;
pub use operation_scope::{AdmittedOperation, KeyScopeRefusal, ResolvedFacetRole};

/// What an API token narrows its user to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenGrant {
    scopes: Scopes,
    subtree: Option<Id>,
    token: Option<Id>,
    kept_out: Vec<KeptOut>,
    /// Explicit shared-operation mode. Legacy permission readers must fail closed for this mode.
    operation_scope: Option<KeyScope>,
    /// Earlier legacy permission ceilings retained by an accepted/current intersection.
    legacy_ceiling: Option<Scopes>,
    /// Conjunctive node bounds which cannot be flattened to one anchor without losing moves.
    other_subtrees: Vec<Id>,
    /// Every actual credential retained by an accepted/current intersection.
    other_tokens: Vec<Id>,
    expires_at: Option<Time>,
}

/// A place a token is kept out of besides what its subtree leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeptOut {
    /// The org refused it, there and in everything inside.
    Refused {
        /// The org.
        org: Id,
    },
    /// Its whole life is longer than the place admits.
    Lifetime {
        /// The org whose bound it is; `None` for the system's, which keeps it out everywhere.
        place: Option<Id>,
        /// The longest life in days the place admits.
        max_days: i64,
    },
}

impl TokenGrant {
    /// A grant of these scopes, kept to the subtree of `subtree` where one
    /// is given.
    #[must_use]
    pub fn new(scopes: Scopes, subtree: Option<Id>) -> Self {
        Self {
            scopes,
            subtree,
            token: None,
            kept_out: Vec::new(),
            operation_scope: None,
            legacy_ceiling: None,
            other_subtrees: Vec::new(),
            other_tokens: Vec::new(),
            expires_at: None,
        }
    }

    /// The same grant, as the grant of the stored token `token`: where it is kept out is read for it
    /// when its principal is loaded.
    #[must_use]
    pub fn for_token(mut self, token: Id) -> Self {
        if let Some(previous) = self.token
            && previous != token
            && !self.other_tokens.contains(&previous)
        {
            self.other_tokens.push(previous);
        }
        self.token = Some(token);
        self.other_tokens.retain(|other| *other != token);
        self
    }

    /// The stored token it is the grant of, where it is one.
    #[must_use]
    pub fn token(&self) -> Option<Id> {
        self.token
    }

    /// The same grant, kept out of these places as well.
    #[must_use]
    pub fn kept_out(mut self, places: Vec<KeptOut>) -> Self {
        for place in places {
            if !self.kept_out.contains(&place) {
                self.kept_out.push(place);
            }
        }
        self
    }

    /// The place, on the way down to the node with the path `path`, that keeps the token out, where
    /// one does: an org on that way that refused it or admits a shorter life, or the system's bound.
    /// A request through no node is kept out only by the system's bound.
    #[must_use]
    pub fn kept_out_of(&self, path: Option<&[Id]>) -> Option<KeptOut> {
        self.kept_out.iter().copied().find(|place| match place {
            KeptOut::Lifetime { place: None, .. } => true,
            KeptOut::Refused { org }
            | KeptOut::Lifetime {
                place: Some(org), ..
            } => path.is_some_and(|path| path.contains(org)),
        })
    }

    /// A token that narrows nothing but the system admin: every scope, no
    /// subtree.
    #[must_use]
    pub fn everything() -> Self {
        Self::new(Scopes::All, None)
    }

    /// The grant of a stored token: its scope words and the node it is
    /// kept to. A word that is no scope is refused, and the token with it.
    pub fn from_stored(scopes: &[String], subtree: Option<Id>) -> Result<Self, UnknownName> {
        Ok(Self::new(Scopes::parse(scopes)?, subtree))
    }

    /// Its scopes.
    #[must_use]
    pub fn scopes(&self) -> &Scopes {
        &self.scopes
    }

    /// The node it is kept to, with everything below.
    #[must_use]
    pub fn subtree(&self) -> Option<Id> {
        // An obsolete serializer must not widen a conjunction of different roots. New
        // continuations use CapturedCaller, which preserves every root and rule layer.
        if self.other_subtrees.is_empty() {
            self.subtree
        } else {
            Some(Id::nil())
        }
    }

    /// Whether the token carries `permission`, where its user holds it.
    /// The scopes are the permissions as the catalogue names them: holding
    /// a level does not put the levels below it into the scopes.
    #[must_use]
    pub fn carries(&self, permission: Permission) -> bool {
        // Operation-mode admission belongs to Caller, never to a reusable credential grant.
        self.operation_scope.is_none() && self.scopes.allow(permission)
    }

    /// Whether the token reaches a request that acts on the node with the
    /// path `path`, the nodes from its root org down to it; `None` for a
    /// request about a resource that goes through no node.
    #[must_use]
    pub fn reaches(&self, path: Option<&[Id]>) -> bool {
        token_reaches(self.subtree, path)
            && self
                .other_subtrees
                .iter()
                .all(|root| token_reaches(Some(*root), path))
            && self.kept_out_of(path).is_none()
    }
}

/// Who asks, and with what.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Caller {
    user: Id,
    /// Verified non-browser machine owner; never an API-token or browser-session identity.
    machine: Option<Id>,
    native_transaction: Option<i64>,
    owner_read: Option<Id>,
    token: Option<TokenGrant>,
    as_admin: bool,
    /// Ephemeral shared-operation evidence for either authentication transport.
    admission: Option<operation_scope::Admission>,
}

impl Caller {
    /// The user `user` in a browser session.
    #[must_use]
    pub fn session(user: Id) -> Self {
        Self {
            user,
            machine: None,
            native_transaction: None,
            owner_read: None,
            token: None,
            as_admin: orch_db::audit::current_origin().as_admin == Some(user),
            admission: None,
        }
    }

    /// The user `user` with an API token that grants `grant`.
    #[must_use]
    pub fn token(user: Id, grant: TokenGrant) -> Self {
        Self {
            user,
            machine: None,
            native_transaction: None,
            owner_read: None,
            token: Some(grant),
            as_admin: false,
            admission: None,
        }
    }

    /// Ordinary non-browser holdings of an owner authenticated by its current machine relation.
    /// The native entry must pin/recheck that relation; there is no HTTP token or admin override.
    #[must_use]
    pub fn machine_owner(machine: Id, user: Id, transaction: i64) -> Self {
        Self {
            user,
            machine: Some(machine),
            native_transaction: Some(transaction),
            owner_read: None,
            token: Some(TokenGrant::everything()),
            as_admin: false,
            admission: None,
        }
    }
    /// A sealed API/native boundary must recheck this owner-only read source at each delivery.
    /// This has no token grant or browser authority and cannot restore outside its live source scope.
    #[must_use]
    pub fn owner_read(user: Id, source: Id) -> Self {
        Self {
            user,
            machine: None,
            native_transaction: None,
            owner_read: Some(source),
            token: None,
            as_admin: false,
            admission: None,
        }
    }

    /// The actual logical native read, separate from either HTTP credential kind.
    #[must_use]
    pub fn owner_read_source(&self) -> Option<Id> {
        self.owner_read
    }

    /// Native machine provenance, independent of browser/API credential facts.
    #[must_use]
    pub fn native_machine(&self) -> Option<Id> {
        self.machine
    }
    /// Native provenance expires with its actual owner-guarded transaction, never a lease.
    #[must_use]
    pub fn native_transaction(&self) -> Option<i64> {
        self.native_transaction
    }

    /// The user.
    #[must_use]
    pub fn user(&self) -> Id {
        self.user
    }

    /// What the request is made with.
    #[must_use]
    pub fn credential(&self) -> Credential {
        if self.owner_read.is_some() {
            Credential::OwnerRead
        } else if self.token.is_some() {
            Credential::Token
        } else {
            Credential::Session
        }
    }

    /// The grant of the token, where there is one.
    #[must_use]
    pub fn grant(&self) -> Option<&TokenGrant> {
        self.token.as_ref()
    }

    /// The same caller, with its token kept out of these places as well; a session is unchanged.
    #[must_use]
    pub fn with_kept_out(mut self, places: Vec<KeptOut>) -> Self {
        self.token = self.token.map(|grant| grant.kept_out(places));
        if !self.admission_is_current() {
            self.admission = None;
        }
        self
    }

    /// Whether what the user holds as a system admin counts: in a browser
    /// session only.
    #[must_use]
    pub fn carries_system_admin(&self) -> bool {
        self.as_admin()
    }

    /// Selects only a browser session's explicit mode; tokens never acquire it.
    #[must_use]
    pub fn with_admin_mode(mut self, enabled: bool) -> Self {
        self.as_admin = enabled && self.token.is_none() && self.owner_read.is_none();
        self
    }

    /// Whether this caller explicitly acts as an administrator.
    #[must_use]
    pub fn as_admin(&self) -> bool {
        self.as_admin && self.token.is_none() && self.owner_read.is_none()
    }

    /// Whether this caller may use `permission` at all, whatever the user
    /// holds: a session may, a token only within its scopes.
    #[must_use]
    pub fn permits(&self, permission: Permission) -> bool {
        self.token.as_ref().is_none_or(|grant| {
            if grant.operation_scope.is_some() {
                self.admission.is_some()
                    && grant
                        .legacy_ceiling
                        .as_ref()
                        .is_none_or(|scopes| scopes.allow(permission))
            } else {
                grant.carries(permission)
            }
        })
    }

    /// Whether this caller reaches a request that acts on the node with
    /// the path `path`: a session does, a token only inside its subtree.
    #[must_use]
    pub fn reaches(&self, path: Option<&[Id]>) -> bool {
        self.token.as_ref().is_none_or(|grant| grant.reaches(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn native_machine_owner_has_no_http_credential_or_admin_override()
    -> Result<(), serde_json::Error> {
        let (machine, user) = (new_id(), new_id());
        let native = Caller::machine_owner(machine, user, 7);
        assert_eq!(native.native_machine(), Some(machine));
        assert_eq!(native.native_transaction(), Some(7));
        assert!(!native.clone().with_admin_mode(true).as_admin());
        assert!(
            native
                .grant()
                .is_some_and(|grant| grant.token_ids().next().is_none())
        );
        assert!(
            native
                .intersect_restrictions(&Caller::session(user))
                .is_err()
        );
        let encoded = serde_json::to_value(CapturedCaller::capture(&native))?;
        assert_eq!(encoded["credentials"][0]["type"], "machine_owner");
        assert_eq!(
            encoded["credentials"][0]["machine"],
            serde_json::json!(machine)
        );
        assert!(encoded["credentials"][0].get("token_ids").is_none());
        Ok(())
    }

    #[test]
    fn a_session_is_narrowed_by_nothing() {
        let caller = Caller::session(new_id());
        assert_eq!(caller.credential(), Credential::Session);
        assert!(
            !caller.carries_system_admin(),
            "new sessions start outside administrator mode"
        );
        assert!(caller.clone().with_admin_mode(true).carries_system_admin());
        assert!(caller.permits(Permission::ThreadInteract));
        assert!(caller.reaches(Some(&[new_id()])));
        assert!(caller.reaches(None));
    }

    #[test]
    fn a_token_never_carries_what_its_user_holds_as_a_system_admin() {
        let caller = Caller::token(new_id(), TokenGrant::everything());
        assert_eq!(caller.credential(), Credential::Token);
        assert!(!caller.carries_system_admin());
        assert!(caller.permits(Permission::OrgManage));
    }

    #[test]
    fn a_token_with_scopes_carries_those_permissions_and_no_others() {
        let grant = TokenGrant::from_stored(
            &["thread.interact".to_owned(), "files.view".to_owned()],
            None,
        )
        .unwrap();
        assert!(grant.carries(Permission::ThreadInteract));
        assert!(grant.carries(Permission::FilesView));
        assert!(
            !grant.carries(Permission::ThreadView),
            "a scope is the permission it names, not the levels below it"
        );
        let all = TokenGrant::from_stored(&["*".to_owned()], None).unwrap();
        assert!(all.carries(Permission::ProviderManage));
    }

    #[test]
    fn a_word_that_is_no_scope_refuses_the_token() {
        assert!(TokenGrant::from_stored(&["node.view".to_owned()], None).is_err());
        assert!(TokenGrant::from_stored(&["everything".to_owned()], None).is_err());
    }

    #[test]
    fn a_token_kept_to_a_subtree_reaches_that_subtree_and_nothing_beside_it() {
        let (root, project, topic, other) = (new_id(), new_id(), new_id(), new_id());
        let grant = TokenGrant::new(Scopes::All, Some(project));
        let caller = Caller::token(new_id(), grant);
        assert!(caller.reaches(Some(&[root, project, topic])));
        assert!(caller.reaches(Some(&[root, project])));
        assert!(!caller.reaches(Some(&[root, other])));
        assert!(!caller.reaches(Some(&[root])));
        assert!(
            !caller.reaches(None),
            "a resource is reached through a node of the subtree or not at all"
        );
    }
}
