//! One native read request over either the admitted Ask transport or a sealed owner tool call.

use super::*;
use crate::middleware::authentication::{Authed, NativeReadIdentity};
use crate::state::App;

#[derive(Clone)]
pub(super) enum ReadContext {
    Ask(Arc<ReadOrigin>),
    Owner {
        app: App,
        identity: NativeReadIdentity,
    },
}

impl ReadContext {
    pub(super) fn app(&self) -> &App {
        match self {
            Self::Ask(origin) => &origin.source.app,
            Self::Owner { app, .. } => app,
        }
    }
    pub(super) fn root(&self) -> Id {
        match self {
            Self::Ask(origin) => origin.root,
            Self::Owner { identity, .. } => identity.root(),
        }
    }
    pub(super) fn conversation(&self) -> Id {
        match self {
            Self::Ask(origin) => origin.conversation,
            Self::Owner { identity, .. } => identity.thread(),
        }
    }
    pub(super) fn owner(&self) -> Id {
        match self {
            Self::Ask(origin) => origin.principal.user(),
            Self::Owner { identity, .. } => identity.owner(),
        }
    }
    pub(super) fn headers(&self) -> Option<&http::HeaderMap> {
        match self {
            Self::Ask(origin) => Some(&origin.source.headers),
            Self::Owner { .. } => None,
        }
    }
    pub(super) fn ceiling(&self) -> orch_authz::Caller {
        match self {
            Self::Ask(origin) => origin.principal.caller().clone().with_admin_mode(false),
            Self::Owner { identity, .. } => {
                orch_authz::Caller::owner_read(identity.owner(), identity.source())
            }
        }
    }
    pub(super) fn router(
        &self,
        registry: crate::registry::Registry,
    ) -> ApiResult<axum::Router<App>> {
        match self {
            Self::Ask(origin) => Ok(registry.into_router_for(origin.source.http_surface()?)),
            Self::Owner { .. } => Ok(registry.into_native_read_router()),
        }
    }
    pub(super) async fn check_owner(&self) -> ApiResult<()> {
        if let Self::Owner { app, identity } = self {
            let mut tx = app.db.begin_snapshot().await?;
            let user = identity.current(&mut tx, app).await?;
            crate::middleware::transaction_guard::guard_user(&user)?;
            let principal = authorization::load_principal(
                &mut tx,
                authorization::caller_of(&user)?,
                app.clock.time(),
            )
            .await?;
            let principal = crate::sign_in::second_step::hold(&mut tx, &user, principal).await?;
            root_access(&mut tx, &principal, identity.root()).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(())
    }
}

/// Only this in-process reader constructs the extension; no header or JSON supplies it.
#[derive(Clone)]
pub(crate) struct ReadRequest {
    context: ReadContext,
    pub(super) selection: String,
    pub(super) sources: Arc<std::sync::Mutex<ReadSources>>,
    resource_context: Option<crate::routes::resources::context::Request>,
}

impl ReadRequest {
    pub(super) fn new(
        context: ReadContext,
        selection: String,
        sources: Arc<std::sync::Mutex<ReadSources>>,
        resource_context: Option<crate::routes::resources::context::Request>,
    ) -> Self {
        Self {
            context,
            selection,
            sources,
            resource_context,
        }
    }
    pub(crate) fn resource_context(&self) -> Option<&crate::routes::resources::context::Request> {
        self.resource_context.as_ref()
    }
    pub(crate) fn http_origin(&self) -> Option<&ReadOrigin> {
        match &self.context {
            ReadContext::Ask(origin) => Some(origin),
            ReadContext::Owner { .. } => None,
        }
    }
    pub(crate) fn allows_read_route(&self, route: &Route) -> ApiResult<bool> {
        orch_api_contract::ask_tools::allows_selection(route, &self.selection)
            .map_err(ApiError::internal)
    }
    pub(crate) fn native_identity(&self) -> Option<&NativeReadIdentity> {
        match &self.context {
            ReadContext::Owner { identity, .. } => Some(identity),
            ReadContext::Ask(_) => None,
        }
    }
    pub(crate) fn ceiling(&self) -> orch_authz::Caller {
        self.context.ceiling()
    }
    pub(crate) async fn native_user(&self, app: &App) -> ApiResult<Authed> {
        let identity = self
            .native_identity()
            .ok_or_else(|| denied("This is not a native owner read"))?;
        let mut tx = app.db.begin_snapshot().await?;
        let user = identity.current(&mut tx, app).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(user)
    }
    pub(crate) fn root(&self) -> Id {
        self.context.root()
    }
    pub(super) fn conversation(&self) -> Id {
        self.context.conversation()
    }
    pub(super) fn owner(&self) -> Id {
        self.context.owner()
    }
}

impl std::fmt::Debug for ReadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReadRequest([private])")
    }
}
