//! Copy-specific source binding, shared by its request, suppliers and durable execution.
use super::Review;
use crate::{
    EngineError,
    operation_scope::{
        continuing::{Error, Source},
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    mount_copy_sources::{self as declarations, Source as Role},
    mounts::{MountCopyCreate, MountCopyPreflight},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{
    Caller, Principal,
    resources::{Locating, LocationAsk, LocationKind},
    usable::Policies,
};
use orch_db::{Conn, DbError, queries::resources::mounts as rows};
use orch_types::Time;

pub struct Preview;
pub struct Confirm;

/// Native identities supply all required roles, never identifiers copied out of a response.
pub async fn facts(conn: &mut Conn, review: &Review) -> Result<Vec<Facts<Role>>, EngineError> {
    subjects(
        conn,
        review.mount.id,
        review.reader.machine,
        review.writer.machine,
    )
    .await
}

pub async fn subjects(
    conn: &mut Conn,
    mount: orch_types::Id,
    reader: orch_types::Id,
    writer: orch_types::Id,
) -> Result<Vec<Facts<Role>>, EngineError> {
    Ok(vec![
        Facts::one(
            Role::Mount,
            KeyTargetKind::Object(ObjectKind::Mount),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: mount,
                },
            )
            .await
            .map_err(EngineError::unreadable)?,
        ),
        Facts::one(
            Role::Reader,
            KeyTargetKind::Object(ObjectKind::Machine),
            targets::machine_subject(conn, reader)
                .await
                .map_err(EngineError::unreadable)?,
        ),
        Facts::one(
            Role::Writer,
            KeyTargetKind::Object(ObjectKind::Machine),
            targets::machine_subject(conn, writer)
                .await
                .map_err(EngineError::unreadable)?,
        ),
    ])
}

pub async fn require(conn: &mut Conn, principal: &Principal, review: &Review) -> Result<(), Error> {
    for (machine, kind, storage) in [
        (
            review.reader.machine,
            if review.source.location.kind == "volume" {
                LocationKind::Volume
            } else {
                LocationKind::Path
            },
            review.source.location.storage.clone(),
        ),
        (review.writer.machine, LocationKind::Volume, None),
    ] {
        let ask = LocationAsk {
            mount: review.mount.id,
            machine,
            kind,
            storage,
        };
        if !matches!(
            orch_authz::resources::may_add_location(conn, principal, &ask, &Policies::default())
                .await?,
            Locating::Allowed
        ) {
            return Err(Error::Refused);
        }
    }
    if review.source.location.kind == "path" {
        let policy: orch_link_contract::policy::LocalPolicy =
            serde_json::from_value(review.reader.policy.clone().ok_or(Error::Refused)?)
                .map_err(EngineError::from)?;
        policy
            .check_root(std::path::Path::new(&review.source.location.source))
            .map_err(|_| Error::Refused)?;
        if review.reader.declaration["data_storage"]["existing_directory"] != true {
            return Err(Error::Refused);
        }
    }
    Ok(())
}

pub async fn lock_source(conn: &mut Conn, review: &Review) -> Result<(), Error> {
    // A short mount UPDATE lock also fences insertion into an absent destination membership.
    rows::lock(conn, review.mount.id)
        .await?
        .ok_or(Error::Refused)?;
    let mut machines = [review.reader.machine, review.writer.machine];
    machines.sort_unstable();
    sqlx::query("SELECT mount_id FROM mount_locations WHERE mount_id=$1 AND machine_id=ANY($2) ORDER BY machine_id FOR UPDATE")
        .bind(review.mount.id).bind(machines.as_slice()).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(machines.as_slice())
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let mut hosts = [review.reader.host, review.writer.host]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    hosts.sort_unstable();
    hosts.dedup();
    sqlx::query("SELECT id FROM hosts WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(&hosts)
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

macro_rules! copy_source {
    ($name:ident, $endpoint:ty, $declaration:ident) => {
        impl Source for $name {
            type Role = Role;
            type Input = Review;
            type Value = Review;
            const ROUTE: Route = <$endpoint>::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &declarations::$declaration;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                review: &'a Review,
                _: Time,
            ) -> BoxFuture<'a, Result<(Review, Vec<Facts<Role>>), Error>> {
                Box::pin(async move {
                    if !review.current(conn).await? {
                        return Err(Error::Refused);
                    }
                    Ok((review.clone(), facts(conn, review).await?))
                })
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                review: &'a Review,
                _: &'a Review,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require(conn, principal, review))
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                review: &'a Review,
                _: &'a Review,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(lock_source(conn, review))
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a Review,
                old: &'a Review,
                fresh: &'a Review,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move { Ok(old == fresh) })
            }
        }
    };
}
copy_source!(Preview, MountCopyPreflight, PREFLIGHT);
copy_source!(Confirm, MountCopyCreate, CREATE);
