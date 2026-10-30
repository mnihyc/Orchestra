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
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{Credential, UnknownName};
use orch_types::Id;

/// What an API token narrows its user to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenGrant {
    scopes: Scopes,
    subtree: Option<Id>,
    token: Option<Id>,
    kept_out: Vec<KeptOut>,
}

/// A place a token is kept out of besides what its subtree leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        }
    }

    /// The same grant, as the grant of the stored token `token`: where it is kept out is read for it
    /// when its principal is loaded.
    #[must_use]
    pub fn for_token(mut self, token: Id) -> Self {
        self.token = Some(token);
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
        self.kept_out = places;
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
        self.subtree
    }

    /// Whether the token carries `permission`, where its user holds it.
    /// The scopes are the permissions as the catalogue names them: holding
    /// a level does not put the levels below it into the scopes.
    #[must_use]
    pub fn carries(&self, permission: Permission) -> bool {
        self.scopes.allow(permission)
    }

    /// Whether the token reaches a request that acts on the node with the
    /// path `path`, the nodes from its root org down to it; `None` for a
    /// request about a resource that goes through no node.
    #[must_use]
    pub fn reaches(&self, path: Option<&[Id]>) -> bool {
        token_reaches(self.subtree, path) && self.kept_out_of(path).is_none()
    }
}

/// Who asks, and with what.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Caller {
    user: Id,
    token: Option<TokenGrant>,
    as_admin: bool,
}

impl Caller {
    /// The user `user` in a browser session.
    #[must_use]
    pub fn session(user: Id) -> Self {
        Self {
            user,
            token: None,
            as_admin: orch_db::audit::current_origin().as_admin == Some(user),
        }
    }

    /// The user `user` with an API token that grants `grant`.
    #[must_use]
    pub fn token(user: Id, grant: TokenGrant) -> Self {
        Self {
            user,
            token: Some(grant),
            as_admin: false,
        }
    }

    /// The user.
    #[must_use]
    pub fn user(&self) -> Id {
        self.user
    }

    /// What the request is made with.
    #[must_use]
    pub fn credential(&self) -> Credential {
        if self.token.is_some() {
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
        self.as_admin = enabled && self.token.is_none();
        self
    }

    /// Whether this caller explicitly acts as an administrator.
    #[must_use]
    pub fn as_admin(&self) -> bool {
        self.as_admin && self.token.is_none()
    }

    /// Whether this caller may use `permission` at all, whatever the user
    /// holds: a session may, a token only within its scopes.
    #[must_use]
    pub fn permits(&self, permission: Permission) -> bool {
        self.token
            .as_ref()
            .is_none_or(|grant| grant.carries(permission))
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
