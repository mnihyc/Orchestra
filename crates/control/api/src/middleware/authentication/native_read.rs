//! A real owner-only native tool source, never a browser session or bearer credential.

use super::*;
use crate::state::App;
use crate::thread_read_tools::OwnerReadPermit;
use orch_db::Conn;

/// Opaque native read identity. Only the checked tool entry can supply its private permit.
#[derive(Clone)]
pub struct NativeReadIdentity {
    permit: Arc<OwnerReadPermit>,
}

impl std::fmt::Debug for NativeReadIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeReadIdentity([private])")
    }
}
impl PartialEq for NativeReadIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.permit, &other.permit)
    }
}
impl Eq for NativeReadIdentity {}

impl NativeReadIdentity {
    pub(crate) fn new(permit: &OwnerReadPermit) -> Self {
        Self {
            permit: Arc::new(permit.clone()),
        }
    }
    pub(crate) fn source(&self) -> Id {
        orch_engine::ids::tool_result(self.permit.thread(), &self.permit.call().call_ref)
    }
    pub(crate) fn owner(&self) -> Id {
        self.permit.owner()
    }
    pub(crate) fn root(&self) -> Id {
        self.permit.root()
    }
    pub(crate) fn thread(&self) -> Id {
        self.permit.thread()
    }

    pub(crate) async fn current(&self, conn: &mut Conn, app: &App) -> ApiResult<Authed> {
        self.permit.check(conn, app).await?;
        let user = identity::user_by_id(conn, self.owner())
            .await?
            .filter(|user| user.status == "active")
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::AuthzDenied,
                    "The native read account is no longer active",
                )
            })?;
        Ok(Authed {
            user,
            as_admin: false,
            via: Via::OwnerRead {
                source: self.clone(),
            },
        })
    }
}
