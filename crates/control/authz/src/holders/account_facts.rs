//! Shared current account facts for permission decisions, without creating another credential.
use super::*;
use std::future::Future;
use std::pin::Pin;

/// Internal data only. An account-eligibility frame cannot become a requester Principal.
#[derive(Clone)]
pub(crate) struct AccountFacts {
    user: Id,
    now: Time,
    pub(super) suspension: Suspension,
    pub(super) flag: bool,
    pub(super) groups: Vec<Id>,
    pub(super) viewers: Vec<Id>,
    pub(super) memberships: Memberships,
    pub(super) member_of: Vec<Id>,
    pub(super) holder_deadline: Option<Time>,
}

impl AccountFacts {
    /// A fixed dependency's ordinary account, with no browser mode or credential authority.
    pub(crate) async fn current(conn: &mut Conn, user: Id, now: Time) -> Result<Self, Error> {
        Self::for_caller(conn, user, now, false).await
    }

    // Only the real Principal loader supplies its actual native reach. Dependency
    // callers cannot select this mode; the account-only entry point above is fixed.
    pub(super) async fn for_caller(
        conn: &mut Conn,
        user: Id,
        now: Time,
        native_reach: bool,
    ) -> Result<Self, Error> {
        let facts = access::principal(conn, user)
            .await?
            .ok_or(Error::NoSuchUser(user))?;
        let memberships = Memberships::load(conn, user).await?;
        let sets =
            kept::holder_sets(conn, user, now, facts.is_system_admin && native_reach).await?;
        let member_of = memberships.member_of();
        Ok(Self {
            user,
            now,
            suspension: Suspension::new(facts.status == "suspended", facts.suspended_in),
            flag: facts.is_system_admin,
            groups: sets.groups.clone(),
            viewers: sets.viewers.clone(),
            memberships,
            member_of,
            holder_deadline: sets.stamp.until().map(Time::new),
        })
    }
}

/// Sealed to this crate: the public Frame keeps Principal as its default subject.
/// Fixed business dependencies may feed current account data to the same evaluator,
/// while no owner Caller, Principal or mode is manufactured or returned.
pub(crate) trait FrameSubject: Clone + Send + Sync {
    fn user(&self) -> Id;
    fn now(&self) -> Time;
    fn suspension(&self) -> &Suspension;
    fn groups(&self) -> &[Id];
    fn is_viewer_of(&self, place: Id) -> bool;
    fn memberships(&self) -> &Memberships;
    fn member_of(&self) -> &[Id];
    fn is_system_admin(&self) -> bool;
    fn private_admin(&self) -> bool;
    fn permits(&self, permission: Permission) -> bool;
    fn reaches(&self, path: Option<&[Id]>) -> bool;
    fn subtree(&self) -> Option<Id>;
    fn ordinary<'a>(
        &'a self,
        conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<Self, Error>> + Send + 'a>>;

    fn is_in_tree(&self, root: Id) -> bool {
        self.memberships().is_in_tree(root)
    }

    fn is_held_by(
        &self,
        holder: &Holder,
        role_at: impl FnOnce(Option<Id>) -> Option<Role>,
    ) -> bool {
        if let Holder::Role {
            role: Role::Viewer,
            at: Some(place),
        } = holder
        {
            return self.is_viewer_of(*place);
        }
        holder.matches(
            &Person {
                id: self.user(),
                groups: self.groups(),
                member_of: self.member_of(),
            },
            role_at,
        )
    }
}

impl FrameSubject for Principal {
    fn user(&self) -> Id {
        Principal::user(self)
    }
    fn now(&self) -> Time {
        Principal::now(self)
    }
    fn suspension(&self) -> &Suspension {
        Principal::suspension(self)
    }
    fn groups(&self) -> &[Id] {
        Principal::groups(self)
    }
    fn is_viewer_of(&self, place: Id) -> bool {
        Principal::is_viewer_of(self, place)
    }
    fn memberships(&self) -> &Memberships {
        Principal::memberships(self)
    }
    fn member_of(&self) -> &[Id] {
        Principal::member_of(self)
    }
    fn is_system_admin(&self) -> bool {
        Principal::is_system_admin(self)
    }
    fn private_admin(&self) -> bool {
        self.caller().as_admin()
    }
    fn permits(&self, permission: Permission) -> bool {
        self.caller().permits(permission)
    }
    fn reaches(&self, path: Option<&[Id]>) -> bool {
        self.caller().reaches(path)
    }
    fn subtree(&self) -> Option<Id> {
        self.caller().grant().and_then(crate::TokenGrant::subtree)
    }
    fn ordinary<'a>(
        &'a self,
        conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<Self, Error>> + Send + 'a>> {
        Box::pin(Principal::ordinary(self, conn))
    }
}

impl FrameSubject for AccountFacts {
    fn user(&self) -> Id {
        self.user
    }
    fn now(&self) -> Time {
        self.now
    }
    fn suspension(&self) -> &Suspension {
        &self.suspension
    }
    fn groups(&self) -> &[Id] {
        &self.groups
    }
    fn is_viewer_of(&self, place: Id) -> bool {
        self.viewers.contains(&place)
    }
    fn memberships(&self) -> &Memberships {
        &self.memberships
    }
    fn member_of(&self) -> &[Id] {
        &self.member_of
    }
    fn is_system_admin(&self) -> bool {
        false
    }
    fn private_admin(&self) -> bool {
        false
    }
    // These facts describe the account before credential narrowing. No public
    // constructor or API exposes an account frame as a requester/effect principal.
    fn permits(&self, _permission: Permission) -> bool {
        true
    }
    fn reaches(&self, _path: Option<&[Id]>) -> bool {
        true
    }
    fn subtree(&self) -> Option<Id> {
        None
    }
    fn ordinary<'a>(
        &'a self,
        _conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<Self, Error>> + Send + 'a>> {
        Box::pin(async move { Ok(self.clone()) })
    }
}
