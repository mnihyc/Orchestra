//! Existing native resource-creation homes and ordinary membership/settings/system authority.

use super::Home;
use super::providers::{home_policies, refused_making};
use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::nodes::{invalid, placed, policy_at};
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::nodes::NodeKind;
use orch_authz::{
    Policy, Principal,
    ownership::{self, Making},
};
use orch_db::{Conn, DbError};
use orch_types::Id;

/// A real selected hierarchy home, or the actual system authority namespace.
pub(super) async fn target(conn: &mut Conn, home: Option<Id>) -> ApiResult<KeyResolvedTarget> {
    match home {
        Some(id) => crate::key_operations::targets::node(conn, id).await,
        None => Ok(crate::key_operations::targets::system()),
    }
}

/// The native creation gate shared by org/system preset and mount creation.
pub(super) async fn require(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    home: Option<Id>,
    invalid_home: &'static str,
    system_only: &'static str,
) -> ApiResult<Home> {
    let (org, members, policy) = match home {
        Some(id) => {
            let (place, node_kind) = placed(conn, id).await?;
            if node_kind != NodeKind::Org {
                return Err(invalid("home_id", invalid_home));
            }
            (
                Some(place.org),
                home_policies(conn, id).await?.members_make,
                policy_at(conn, id).await?,
            )
        }
        None => (None, Vec::new(), Policy::default()),
    };
    match ownership::may_make(conn, principal, kind, home, &members, policy).await {
        Ok(Making::Allowed(_)) => Ok(Home { node: home, org }),
        Ok(Making::Refused(why)) => Err(match home {
            Some(id) => refused_making(conn, id, why).await,
            None => refusals::system_admin_only(system_only),
        }),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

/// The insert's real home foreign-key and lifecycle serialization footprint.
pub(super) async fn lock(conn: &mut Conn, home: Option<Id>) -> ApiResult<()> {
    if let Some(id) = home {
        let (place, _) = placed(conn, id).await?;
        let mut ids = vec![id, place.org];
        ids.sort_unstable();
        ids.dedup();
        let selected: Vec<Id> =
            sqlx::query_scalar("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(&ids)
                .fetch_all(conn)
                .await
                .map_err(DbError::from)?;
        if selected != ids {
            return Err(ApiError::not_found());
        }
    }
    Ok(())
}
