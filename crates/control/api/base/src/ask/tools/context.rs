//! One native read request over either the admitted Ask transport or a sealed owner tool call.

use super::*;

use crate::middleware::authentication::Authed;

use crate::middleware::authentication::NativeReadIdentity;

use crate::state::App;
#[derive(Clone)]
pub enum ReadContext {
    Ask(Arc<ReadOrigin>),
    Owner {
        app: App,
        identity: NativeReadIdentity,
    },
}

impl ReadContext {
    pub fn app(&self) -> &App {
        match self {
            Self::Ask(origin) => &origin.source.app,
            Self::Owner { app, .. } => app,
        }
    }
    pub fn root(&self) -> Id {
        match self {
            Self::Ask(origin) => origin.root,
            Self::Owner { identity, .. } => identity.root(),
        }
    }
    pub fn conversation(&self) -> Id {
        match self {
            Self::Ask(origin) => origin.conversation,
            Self::Owner { identity, .. } => identity.thread(),
        }
    }
    pub fn owner(&self) -> Id {
        match self {
            Self::Ask(origin) => origin.principal.user(),
            Self::Owner { identity, .. } => identity.owner(),
        }
    }
    pub fn headers(&self) -> Option<&http::HeaderMap> {
        match self {
            Self::Ask(origin) => Some(&origin.source.headers),
            Self::Owner { .. } => None,
        }
    }
    pub fn ceiling(&self) -> orch_authz::Caller {
        match self {
            Self::Ask(origin) => origin.principal.caller().clone().with_admin_mode(false),
            Self::Owner { identity, .. } => {
                orch_authz::Caller::owner_read(identity.owner(), identity.source())
            }
        }
    }
    pub fn router(&self, registry: crate::registry::Registry) -> ApiResult<axum::Router<App>> {
        match self {
            Self::Ask(origin) => Ok(registry.into_router_for(origin.source.http_surface()?)),
            Self::Owner { .. } => Ok(registry.into_native_read_router()),
        }
    }
    pub async fn check_owner(&self) -> ApiResult<()> {
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
pub struct ReadRequest {
    context: ReadContext,
    pub(super) selection: String,
    pub(super) sources: Arc<std::sync::Mutex<ReadSources>>,
}

impl ReadRequest {
    pub fn new(
        context: ReadContext,
        selection: String,
        sources: Arc<std::sync::Mutex<ReadSources>>,
    ) -> Self {
        Self {
            context,
            selection,
            sources,
        }
    }
    pub fn http_origin(&self) -> Option<&ReadOrigin> {
        match &self.context {
            ReadContext::Ask(origin) => Some(origin),
            ReadContext::Owner { .. } => None,
        }
    }
    pub fn native_identity(&self) -> Option<&NativeReadIdentity> {
        match &self.context {
            ReadContext::Owner { identity, .. } => Some(identity),
            ReadContext::Ask(_) => None,
        }
    }
    pub fn ceiling(&self) -> orch_authz::Caller {
        self.context.ceiling()
    }
    pub async fn native_user(&self, app: &App) -> ApiResult<Authed> {
        let identity = self
            .native_identity()
            .ok_or_else(|| denied("This is not a native owner read"))?;
        let mut tx = app.db.begin_snapshot().await?;
        let user = identity.current(&mut tx, app).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(user)
    }
    pub fn root(&self) -> Id {
        self.context.root()
    }
    pub fn conversation(&self) -> Id {
        self.context.conversation()
    }
    pub fn owner(&self) -> Id {
        self.context.owner()
    }
}

impl std::fmt::Debug for ReadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReadRequest([private])")
    }
}
