//! Native complete document metadata, selected without borrowing an endpoint's admission.

use crate::middleware::requests::ApiResult;

use crate::routes::access::page::NameBounds;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

pub use orch_api_base::routes::documents::metadata_projection::Prepared;
/// The original neutral single-document live-row/standing prefix, with no containing-node View gate.
pub(crate) async fn prepare(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Prepared> {
    let row = super::live(conn, id).await?;
    let listed = super::build::standing(conn, principal, id, Policy::default()).await?;
    Ok(Prepared {
        row,
        listed,
        bounds: NameBounds::default(),
    })
}
orch_api_base::__api_bind_205f3cab3b!();
