//! A provider owner's current ordinary Host Use, independent of who manages its runtime.
use super::*;
use crate::holders::AccountFacts;
use orch_types::Time;

/// Checks one fixed business dependency, not another acting credential. The owner
/// is read from this actual provider and no account frame or Principal escapes.
/// Requester Provider Manage and any accepted key bounds remain separate checks.
pub async fn provider_owner_host_use(
    conn: &mut Conn,
    provider: Id,
    host: Id,
    now: Time,
) -> Result<bool, Error> {
    let (_, provider, _) = match site(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    )
    .await
    {
        Ok(current) => current,
        Err(Error::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(owner) = provider.first_owner else {
        return Ok(false);
    };
    let account = match AccountFacts::current(conn, owner, now).await {
        Ok(current) => current,
        Err(Error::NoSuchUser(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    let (graph, resource, at) = match site(
        conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
    )
    .await
    {
        Ok(current) => current,
        Err(Error::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    // The existing resource Use rule excludes unavailable resources. Their pages
    // remain manageable; this adds no provider status, machine or placement veto.
    if resource.deleted || resource.disabled {
        return Ok(false);
    }
    let mut wanted = vec![Permission::HostUse];
    wanted.extend(Permission::HostUse.held_through());
    let frame = Frame::load_with_participation(
        conn,
        &account,
        Policy::default(),
        graph,
        std::slice::from_ref(&at),
        &wanted,
        &[],
    )
    .await?;
    Ok(!frame.account_ways(&at, Permission::HostUse).is_empty())
}
