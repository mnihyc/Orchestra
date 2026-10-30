//! Bringing the sharing a package carries: the people it names on a place or a document that the import made, given again by the person who imports through the access engine, so that what they may give, the locks above and the limits of sharing hold as they do for any change of a page.
//!
//! Nothing is written to a page except by that engine: a person who holds less than what a row gives, a row decided by a lock above, a
//! person from outside the root org that the place does not let be named, all come back as a refusal, which is one line of the
//! report and nothing else. Only the things this import made are shared again: what was there keeps the sharing it has.

use super::{Batch, Made, MakeError};
use crate::error::EngineError;
use crate::imports::model::Object;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::pages::{EntryAdd, Holder, PageChange, RowChange};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::package::{PackageAccess, PackageKind};
use orch_authz::Policy;
use orch_authz::pages::{self, ChangeError, Target};
use orch_authz::usable::ResourcePolicy;
use orch_db::audit::Origin;
use orch_db::{Conn, DbError, Tx};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use std::collections::BTreeMap;

/// What the settings say about sharing at a place: whether its owners may name somebody from outside the root org, and whether
/// somebody who holds what they share without owning it may share it.
async fn policy_at(conn: &mut Conn, node: Id) -> Result<Policy, MakeError> {
    let external = orch_settings::get::<bool>(
        conn,
        orch_settings::Place::Node(node),
        keys::SHARING_ALLOW_EXTERNAL,
    )
    .await
    .map_err(EngineError::from)?;
    let holders = orch_settings::get::<bool>(
        conn,
        orch_settings::Place::Node(node),
        keys::SHARING_ALLOW_HOLDERS,
    )
    .await
    .map_err(EngineError::from)?;
    Ok(Policy {
        owners_name_outsiders: external.unwrap_or(true),
        holders_share: holders.unwrap_or(true),
    })
}

/// What the package carries for `object` that can be given again here: by permission, the people it names who are accounts here.
fn wanted_for(batch: &Batch<'_>, object: &Object) -> (BTreeMap<String, Vec<EntryAdd>>, i64) {
    let mut wanted: BTreeMap<String, Vec<EntryAdd>> = BTreeMap::new();
    let mut left = 0_i64;
    let Some(all) = &batch.plan.opened.access else {
        return (wanted, left);
    };
    let importer = batch.principal.user();
    for row in all
        .iter()
        .filter(|row: &&PackageAccess| row.object == object.object.id)
    {
        let Some(user) = batch.plan.credits.account(row.principal) else {
            left += 1;
            continue;
        };
        if user == importer {
            // They own what they made; nothing is to be given them.
            continue;
        }
        if row.permission.parse::<Permission>().is_err() {
            left += 1;
            continue;
        }
        wanted
            .entry(row.permission.clone())
            .or_default()
            .push(EntryAdd {
                holder: Holder::User { id: user },
                options: None,
                expires_at: row.expires_at,
                note: (!row.note.is_empty()).then(|| row.note.clone()),
            });
    }
    (wanted, left)
}

/// Gives again the sharing that the package carries for `object`, which this import made as `target`. A row that is refused is a
/// warning of the thing, not a failure of it.
pub(super) async fn restore(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
    target: Id,
    made: &mut Made,
) -> Result<(), MakeError> {
    if !batch.plan.access.restored {
        return Ok(());
    }
    let place = matches!(
        object.made_as,
        PackageKind::Org | PackageKind::Project | PackageKind::Topic | PackageKind::Thread
    );
    if !place && object.made_as != PackageKind::Document {
        return Ok(());
    }
    let (wanted, mut left) = wanted_for(batch, object);
    if wanted.is_empty() && left == 0 {
        return Ok(());
    }
    let policy = policy_at(tx, object.lies_in).await?;
    let origin = Origin::default();
    for (permission, adds) in wanted {
        let count = i64::try_from(adds.len()).unwrap_or(i64::MAX);
        let change = PageChange {
            rows: Some(vec![RowChange {
                permission,
                profile: None,
                inherit: None,
                locked: None,
                add: Some(adds),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        };
        sqlx::query("SAVEPOINT import_access")
            .execute(&mut **tx)
            .await
            .map_err(DbError::from)?;
        let result = if place {
            pages::change(
                tx,
                batch.principal,
                Target::Live(target),
                &change,
                policy,
                &origin,
            )
            .await
        } else {
            pages::change_resource(
                tx,
                batch.principal,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id: target,
                },
                &change,
                &ResourcePolicy {
                    owners_name_outsiders: policy.owners_name_outsiders,
                    shared_subscriptions: None,
                },
                &origin,
            )
            .await
        };
        match result {
            Ok(_) => {}
            Err(ChangeError::Engine(error)) => {
                sqlx::query("ROLLBACK TO SAVEPOINT import_access")
                    .execute(&mut **tx)
                    .await
                    .map_err(DbError::from)?;
                sqlx::query("RELEASE SAVEPOINT import_access")
                    .execute(&mut **tx)
                    .await
                    .map_err(DbError::from)?;
                return Err(error.into());
            }
            Err(_) => {
                // Refused: by what the person holds, by a lock above, or by what the place lets be named.
                sqlx::query("ROLLBACK TO SAVEPOINT import_access")
                    .execute(&mut **tx)
                    .await
                    .map_err(DbError::from)?;
                left += count;
            }
        }
        sqlx::query("RELEASE SAVEPOINT import_access")
            .execute(&mut **tx)
            .await
            .map_err(DbError::from)?;
    }
    if left > 0 {
        made.warn(
            "sharing_not_restored",
            format!("{left} shares of it could not be given here: the person is not known here, or what the place allows does not let it be given"),
        );
    }
    Ok(())
}
