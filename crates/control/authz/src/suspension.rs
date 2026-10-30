//! A user suspended altogether, or a person suspended in one org: nothing held where it applies, and nothing passed on.
//!
//! Suspending is how a door is closed at once, before anybody has decided
//! what becomes of a person's things. Altogether, it is a system admin's:
//! nothing is held anywhere. In an org it is that org's admins': nothing is
//! held in that org and in what is inside it, and nothing beside or above
//! it is touched. What a suspended person owns stays theirs, and lifting
//! the suspension puts everything back because nothing was removed.

use orch_access_contract::roles::is_suspended;
use orch_types::Id;

/// Where a person is suspended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suspension {
    altogether: bool,
    in_orgs: Vec<Id>,
    /// Root orgs the request is held out of until the person's sign-in has the second step they
    /// require ([`SecondStepHold`]): closed like a suspension, though nobody suspended them.
    held: Vec<Id>,
}

/// Where a request is held out until its person's sign-in has a second step: the root orgs that
/// require one, with everything inside them, while no second proof is set up, or while this browser
/// session has not given the one that is. Nothing on any page changes: the hold is lifted as soon as
/// the step is set up or given, and their own org and everything outside these orgs stay open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecondStepHold {
    /// The root orgs that require it, held with everything inside them.
    pub orgs: Vec<Id>,
    /// No second proof is set up yet; otherwise one is, and this session has not given it.
    pub setup: bool,
    /// Where the requirement is set: an org, or `None` for the system.
    pub sources: Vec<Option<Id>>,
    /// The second proofs the person has set up, as a sign-in names them (`totp`, `email_code`).
    pub methods: Vec<String>,
}

/// What closes a thing to a suspended person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Closing {
    /// The user is suspended on the whole system.
    Altogether,
    /// They are suspended in this org, which the thing lies in.
    Org(Id),
}

impl Closing {
    /// The org, where it is one: what a refusal names.
    #[must_use]
    pub fn org(self) -> Option<Id> {
        match self {
            Self::Altogether => None,
            Self::Org(org) => Some(org),
        }
    }
}

impl Suspension {
    /// A person suspended `altogether`, and in the orgs `in_orgs`.
    #[must_use]
    pub fn new(altogether: bool, in_orgs: Vec<Id>) -> Self {
        Self {
            altogether,
            in_orgs,
            held: Vec::new(),
        }
    }

    /// The same, held out of the root orgs `held` as well until a second step is set up or given.
    #[must_use]
    pub fn with_held(mut self, held: Vec<Id>) -> Self {
        self.held = held;
        self
    }

    /// The root orgs held out until a second step.
    #[must_use]
    pub fn held(&self) -> &[Id] {
        &self.held
    }

    /// Whether the user is suspended on the whole system.
    #[must_use]
    pub fn altogether(&self) -> bool {
        self.altogether
    }

    /// The orgs they are suspended in.
    #[must_use]
    pub fn in_orgs(&self) -> &[Id] {
        &self.in_orgs
    }

    /// Whether a thing that lies in the orgs `orgs`, from its root org
    /// inward, is closed to them.
    #[must_use]
    pub fn applies(&self, orgs: &[Id]) -> bool {
        is_suspended(self.altogether, &self.in_orgs, orgs)
            || orgs.iter().any(|org| self.held.contains(org))
    }

    /// What closes a thing that lies in `orgs` to them, if anything: the
    /// suspension altogether, or the outermost org of the thing's path they
    /// are suspended in (or held out of until a second step), which is where
    /// a refusal names them.
    #[must_use]
    pub fn closing(&self, orgs: &[Id]) -> Option<Closing> {
        if self.altogether {
            return Some(Closing::Altogether);
        }
        orgs.iter()
            .find(|org| self.in_orgs.contains(org) || self.held.contains(org))
            .map(|org| Closing::Org(*org))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn a_suspension_in_an_org_closes_that_org_and_what_is_inside_it_and_names_the_outermost() {
        let (root, research, ml, sales) = (new_id(), new_id(), new_id(), new_id());
        let suspension = Suspension::new(false, vec![ml, research]);
        assert_eq!(
            suspension.closing(&[root, research, ml]),
            Some(Closing::Org(research)),
            "of two orgs on the way, the one further out"
        );
        assert_eq!(
            suspension.closing(&[root, research]),
            Some(Closing::Org(research))
        );
        assert_eq!(
            suspension.closing(&[root]),
            None,
            "the org above it is open"
        );
        assert_eq!(
            suspension.closing(&[root, sales]),
            None,
            "a sibling is open"
        );
        assert!(suspension.applies(&[root, research, ml]));
        assert!(!suspension.applies(&[root, sales]));
    }

    #[test]
    fn a_root_org_held_for_a_second_step_is_closed_like_a_suspension_and_the_rest_stays_open() {
        let (acme, research, personal, other) = (new_id(), new_id(), new_id(), new_id());
        let held = Suspension::new(false, Vec::new()).with_held(vec![acme]);
        assert_eq!(held.closing(&[acme, research]), Some(Closing::Org(acme)));
        assert!(held.applies(&[acme]));
        assert!(!held.applies(&[personal]), "their own org stays open");
        assert!(
            !held.applies(&[other]),
            "an org that requires nothing stays open"
        );
        assert!(held.in_orgs().is_empty(), "nobody suspended them");
        assert_eq!(held.held(), [acme]);
    }

    #[test]
    fn a_suspension_altogether_closes_everything() {
        let suspension = Suspension::new(true, Vec::new());
        assert_eq!(suspension.closing(&[new_id()]), Some(Closing::Altogether));
        assert_eq!(Closing::Altogether.org(), None);
        assert!(suspension.applies(&[]));
        assert!(!Suspension::default().applies(&[new_id()]));
    }
}
