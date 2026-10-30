//! Handing a thing over: the first place among its owners goes to another person, who must be an active member of its root org, and the person who was first stays an owner unless the same step takes them out.
//!
//! It is the ordinary change of the Owners row, so it is made by one of the owners or by an admin of
//! the place the thing is in, is audited as the passing on of a node, a resource or a document, and
//! can never leave a thing without an owner: the person it goes to is named in the row in the same
//! step, and whoever stays in the row is still there. What was registered from a person's machine
//! keeps that machine's owner as its first owner; further owners are added to its page as to any.

use orch_access_contract::pages::{Holder, OwnersChange, PageChange};
use orch_access_contract::permissions::Permission;
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
        return Ok(Handed {
            changed: false,
            from: previous,
            to,
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
    let outcome: Outcome = match thing {
        Thing::Node(_) => {
            pages::change(
                conn,
                principal,
                thing.target(),
                &change,
                policy.node_policy(),
                origin,
            )
            .await?
        }
        Thing::Other(object) => {
            pages::change_resource(conn, principal, object, &change, policy, origin).await?
        }
    };
    Ok(Handed {
        changed: outcome.changed,
        from: previous,
        to,
        previous_stays,
    })
}
