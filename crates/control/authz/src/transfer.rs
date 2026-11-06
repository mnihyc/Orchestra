//! Handing a thing over: the first place among its owners goes to another person, who must be an active member of its root org, and the person who was first stays an owner unless the same step takes them out.
//!
//! It is the ordinary change of the Owners row, so it is made by one of the owners or by an admin of
//! the place the thing is in, is audited as the passing on of a node, a resource or a document, and
//! can never leave a thing without an owner: the person it goes to is named in the row in the same
//! step, and whoever stays in the row is still there. What was registered from a person's machine
//! keeps that machine's owner as its first owner; further owners are added to its page as to any.

use orch_access_contract::pages::{Holder, OwnersChange, PageChange};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_db::Conn;
use orch_db::audit::Origin;
use orch_db::queries::access;
use orch_types::Id;

use crate::engine::Frame;
use crate::nodes::every_permission;
use crate::pages::{self, ChangeError, Outcome};
use crate::things::{self, Thing};
use crate::usable::ResourcePolicy;
use crate::{Error, Principal, missing};

/// Why a thing could not be handed over.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The access engine failed, or there is no such thing.
    #[error(transparent)]
    Engine(#[from] Error),
    /// There is no such person to hand it to.
    #[error("there is no such person")]
    NoSuchUser,
    /// The person is not an active member of the root org the thing is in: they are suspended, or
    /// they are of another org, and an owner is a member.
    #[error("the person is not an active member of the org the thing lies in")]
    NotAnActiveMember,
    /// The change of the Owners row was refused.
    #[error("the change of the owners was refused")]
    Change(Box<ChangeError>),
}

impl From<ChangeError> for TransferError {
    fn from(error: ChangeError) -> Self {
        Self::Change(Box::new(error))
    }
}

impl From<orch_db::DbError> for TransferError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Engine(error.into())
    }
}

/// What a handing over came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handed {
    /// Whether anything changed: handing a thing to the person who is first already changes nothing.
    pub changed: bool,
    /// The first owner before.
    pub from: Option<Id>,
    /// The first owner now.
    pub to: Id,
    /// Whether the person who was first is still an owner by their own entry.
    pub previous_stays: bool,
}

/// The ordinary handover's effect-free materialization. It carries facts and the
/// already constructed native Owners change, never an authorization grant or wire token.
pub struct PreparedHandover {
    thing: Thing,
    kind: ObjectKind,
    from: Option<Id>,
    to: Id,
    keep_previous: bool,
    change: Option<PageChange>,
    previous_stays: bool,
}
impl PreparedHandover {
    /// The original no-op takes no page target lock.
    pub fn mutation_target(&self) -> Option<crate::pages::Target> {
        self.change.as_ref().map(|_| self.thing.target())
    }
    fn same_facts(&self, current: &Self) -> bool {
        self.thing == current.thing
            && self.kind == current.kind
            && self.from == current.from
            && self.to == current.to
            && self.keep_previous == current.keep_previous
            && self.change.is_some() == current.change.is_some()
    }
}

/// The same native eligibility, receiver checks and ownership change preparation.
/// It performs no write; a no-op remains distinguishable before target serialization.
pub async fn prepare_hand_over(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    to: Id,
    keep_previous: bool,
    policy: &ResourcePolicy,
) -> Result<PreparedHandover, TransferError> {
    let (graph, site) = things::site(conn, thing).await?;
    // Whether the caller may hand it over is settled before anything is said of the person it goes
    // to: one of its owners or an admin of the place it is in, as for any change of the Owners row.
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        principal,
        policy.node_policy(),
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    if !(frame.owns(&site) || frame.administers(&site)) {
        let refusal = missing::refusal(
            conn,
            &frame,
            &site,
            Permission::AccessOwn,
            &[Permission::AccessOwn],
        )
        .await?;
        return Err(ChangeError::Denied(Box::new(refusal)).into());
    }
    let candidate = access::principal(conn, to)
        .await?
        .ok_or(TransferError::NoSuchUser)?;
    // Active: not suspended altogether, nor in an org the thing lies in; a member of the root org.
    let active = candidate.status == "active"
        && !access::active_among(conn, &[to], &site.orgs)
            .await?
            .is_empty()
        && match site.root {
            Some(root) => !access::members_among(conn, root, &[to]).await?.is_empty(),
            None => true,
        };
    if !active {
        return Err(TransferError::NotAnActiveMember);
    }
    let previous = site.first_owner;
    if previous == Some(to) {
        return Ok(PreparedHandover {
            thing,
            kind: site.kind,
            from: previous,
            to,
            keep_previous,
            change: None,
            previous_stays: true,
        });
    }
    let mut remove = Vec::new();
    if !keep_previous && let Some(previous) = previous {
        let key = site.page().key();
        for entry in access::owner_entries(conn, std::slice::from_ref(&key)).await? {
            if entry.holder_kind == "user" && entry.holder_id == Some(previous) {
                remove.push(entry.id);
            }
        }
    }
    let previous_stays = remove.is_empty();
    let change = PageChange {
        owners: Some(OwnersChange {
            add: Some(vec![Holder::User { id: to }]),
            remove: (!remove.is_empty()).then_some(remove),
            first: Some(to),
        }),
        ..PageChange::default()
    };
    Ok(PreparedHandover {
        thing,
        kind: site.kind,
        from: previous,
        to,
        keep_previous,
        change: Some(change),
        previous_stays,
    })
}

/// Makes `to` the first owner of `thing`: named in its Owners row and first in it from now on.
/// The person who was first stays in the row unless `keep_previous` is false, in which case their
/// own entry is taken out in the same step. `policy` is what the settings say at the thing's home.
pub async fn hand_over(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    to: Id,
    keep_previous: bool,
    policy: &ResourcePolicy,
    origin: &Origin,
) -> Result<Handed, TransferError> {
    let prepared = prepare_hand_over(conn, principal, thing, to, keep_previous, policy).await?;
    apply_hand_over(conn, principal, prepared, policy, origin).await
}

/// Re-evaluate the same ordinary preparation after the entry owner's actual HTTP
/// checkpoint. Changed owner/no-op facts cannot create an unrefreshed new mutation.
pub async fn hand_over_prepared_fenced<E: Send, F: FnOnce() -> Result<(), E> + Send>(
    conn: &mut Conn,
    principal: &Principal,
    prepared: PreparedHandover,
    policy: &ResourcePolicy,
    origin: &Origin,
    before_effect: F,
) -> Result<Handed, pages::EffectFailure<TransferError, E>> {
    let current = prepare_hand_over(
        conn,
        principal,
        prepared.thing,
        prepared.to,
        prepared.keep_previous,
        policy,
    )
    .await
    .map_err(pages::EffectFailure::Native)?;
    if !prepared.same_facts(&current) {
        return Err(pages::EffectFailure::Native(orch_db::DbError::Retry.into()));
    }
    apply_hand_over_fenced(conn, principal, current, policy, origin, before_effect).await
}

async fn apply_hand_over(
    conn: &mut Conn,
    principal: &Principal,
    prepared: PreparedHandover,
    policy: &ResourcePolicy,
    origin: &Origin,
) -> Result<Handed, TransferError> {
    match apply_hand_over_fenced(conn, principal, prepared, policy, origin, || {
        Ok::<(), std::convert::Infallible>(())
    })
    .await
    {
        Ok(handed) => Ok(handed),
        Err(pages::EffectFailure::Native(error)) => Err(error),
        Err(pages::EffectFailure::BeforeEffect(never)) => match never {},
    }
}

async fn apply_hand_over_fenced<E: Send, F: FnOnce() -> Result<(), E> + Send>(
    conn: &mut Conn,
    principal: &Principal,
    prepared: PreparedHandover,
    policy: &ResourcePolicy,
    origin: &Origin,
    before_effect: F,
) -> Result<Handed, pages::EffectFailure<TransferError, E>> {
    let Some(change) = prepared.change else {
        return Ok(Handed {
            changed: false,
            from: prepared.from,
            to: prepared.to,
            previous_stays: true,
        });
    };
    let thing = prepared.thing;
    let outcome: Outcome = match thing {
        Thing::Node(_) => {
            pages::change_fenced(
                conn,
                principal,
                thing.target(),
                &change,
                policy.node_policy(),
                origin,
                before_effect,
            )
            .await
        }
        Thing::Other(object) => {
            pages::change_resource_under_fenced(
                conn,
                principal,
                object,
                &change,
                policy.node_policy(),
                policy,
                origin,
                before_effect,
            )
            .await
        }
    }
    .map_err(|error| match error {
        pages::ChangeFailure::Native(error) => {
            pages::EffectFailure::Native(TransferError::from(error))
        }
        pages::ChangeFailure::BeforeEffect(error) => pages::EffectFailure::BeforeEffect(error),
    })?;
    Ok(Handed {
        changed: outcome.changed,
        from: prepared.from,
        to: prepared.to,
        previous_stays: prepared.previous_stays,
    })
}
