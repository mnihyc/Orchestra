//! Native singleton cardinality, including individual styles and metadata generation cuts.

pub use super::*;

pub use crate::routes::document_tree;

pub use crate::routes::documents;

pub use orch_access_contract::permissions::ObjectKind as K;

pub use orch_api_contract::resources::Resource;

pub use orch_types::Time;

use std::sync::Arc;
pub enum Body {
    Resource(Box<Prepared>),
    Document(Box<documents::metadata_projection::Prepared>),
    Folder(orch_engine::document_tree::TreeRead),
}

pub struct Single {
    pub object: ObjectRef,
    pub body: Body,
}

/// The consumer's declared source slots; these are catalogue roles, never permissions.
pub struct Roles {
    pub names: NameRoles,
    pub sources: &'static str,
}

/// Actual native content origins, independent of response DTOs and operation admission.
#[derive(Clone, Copy)]
pub struct Origin {
    pub object: ObjectRef,
    pub retained: bool,
}

pub struct Bounds {
    pub names: NameSources,
    pub origins: Vec<Origin>,
    pub roles: &'static Roles,
}

pub struct Snapshot {
    pub value: Resource,
    pub object: ObjectRef,
    pub cut: Option<document_tree::metadata::Cut>,
    pub native: Option<Arc<Prepared>>,
}

/// The original metadata generation without retaining or cloning the rendered body.
#[derive(Clone)]
pub struct Validation {
    pub cut: Option<document_tree::metadata::Cut>,
    pub object: ObjectRef,
    pub native: Option<Arc<Prepared>>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_227c63a964, __api_install_227c63a964, __api_callback_227c63a964;
    slot __ApiCallback227c63a964, __API_CALLBACK_227C63A964, __api_set_227c63a964;
    base orch_api_base::routes::resources::projection::single;

    async fn subject<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_access_contract::decisions::ObjectRef) -> orch_api_base::middleware::requests::ApiResult<orch_access_contract::key_scope::KeyResolvedTarget>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_dbb26ef146, __api_install_dbb26ef146, __api_callback_dbb26ef146;
    slot __ApiCallbackdbb26ef146, __API_CALLBACK_DBB26EF146, __api_set_dbb26ef146;
    base orch_api_base::routes::resources::projection::single;

    async fn prepare<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_access_contract::decisions::ObjectRef) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::projection::single::Single>;
}

impl Single {
    pub async fn names(&self, conn: &mut Conn) -> ApiResult<NameSources> {
        Ok(match &self.body {
            Body::Resource(native) => native.name_sources().clone(),
            Body::Document(native) => native.names(conn).await?,
            Body::Folder(native) => NameSources {
                containers: native
                    .ancestors
                    .iter()
                    .filter(|row| row.title.is_some() || row.basename.is_some())
                    .map(|row| row.id)
                    .collect(),
                ..Default::default()
            },
        })
    }
    pub fn origins(&self) -> Vec<Origin> {
        match &self.body {
            Body::Resource(native) => native
                .foreign
                .iter()
                .map(|object| Origin {
                    object: *object,
                    retained: native.retained.contains(&(object.kind, object.id)),
                })
                .collect(),
            Body::Document(_) | Body::Folder(_) => Vec::new(),
        }
    }
    pub async fn bind(
        &mut self,
        conn: &mut Conn,
        caller: &Caller,
        plan: &mut OperationPlan,
        roles: &'static Roles,
    ) -> ApiResult<Bounds> {
        let names = self.names(conn).await?;
        let bounds = projection_names(conn, caller, plan, names.clone(), &roles.names).await?;
        let used = NameSources {
            groups: names.groups,
            nodes: bounds.nodes.clone(),
            containers: bounds.containers.clone(),
            ..Default::default()
        };
        match &mut self.body {
            Body::Resource(native) => native.bound_names(bounds),
            Body::Document(native) => native.bound_names(bounds),
            Body::Folder(native) if bounds.limited => {
                for ancestor in &mut native.ancestors {
                    if !bounds.containers.contains(&ancestor.id) {
                        ancestor.title = None;
                        ancestor.basename = None;
                    }
                }
            }
            Body::Folder(_) => {}
        }
        let origins = self.origins();
        bind_origins(conn, plan, roles.sources, &origins).await?;
        Ok(Bounds {
            names: used,
            origins,
            roles,
        })
    }
    pub async fn render(
        self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Snapshot> {
        let mut current = ctx.clone();
        current.now = principal.now().get();
        let ctx = &current;
        let (value, node, native) = match self.body {
            Body::Resource(native) => {
                let native = Arc::new(*native);
                (
                    resource(native.render(ctx, conn, principal).await?)?,
                    None,
                    Some(native),
                )
            }
            Body::Document(native) => {
                let node = native.node();
                (
                    Resource::Document(native.render(conn, principal, Time::new(ctx.now)).await?),
                    Some(node),
                    None,
                )
            }
            Body::Folder(native) => {
                let node = native
                    .entries
                    .first()
                    .map(|row| row.entry.node)
                    .ok_or_else(ApiError::not_found)?;
                (
                    Resource::DocumentContainer(document_tree::convert(native)?),
                    Some(node),
                    None,
                )
            }
        };
        let cut = match node {
            Some(node) => Some(
                document_tree::metadata::cut(conn, principal, node, ctx.app.clock.time()).await?,
            ),
            None => None,
        };
        Ok(Snapshot {
            value,
            object: self.object,
            cut,
            native,
        })
    }
}

impl Bounds {
    /// Refresh only the exact sources already emitted; current bounds cannot relabel old content.
    pub async fn refresh(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        plan: &mut OperationPlan,
    ) -> ApiResult<bool> {
        let fresh =
            projection_names(conn, caller, plan, self.names.clone(), &self.roles.names).await?;
        if !self.names.nodes.is_subset(&fresh.nodes)
            || !self.names.containers.is_subset(&fresh.containers)
        {
            return Ok(false);
        }
        bind_origins(conn, plan, self.roles.sources, &self.origins).await?;
        Ok(true)
    }
}

impl Snapshot {
    pub fn validation(&self) -> Validation {
        Validation {
            cut: self.cut.clone(),
            object: self.object,
            native: self.native.clone(),
        }
    }
    pub fn object(&self) -> ObjectRef {
        self.object
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn has_cut(&self) -> bool {
        self.cut.is_some()
    }
    /// A metadata snapshot is committed before its current-generation validation begins.
    pub async fn after_snapshot(&self) {
        #[cfg(any(test, feature = "test-support"))]
        tests::after_snapshot(self).await;
    }
}

impl Validation {
    pub async fn current(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        now: Time,
    ) -> ApiResult<bool> {
        if let Some(opened) = &self.native {
            let fresh = prepare(ctx, conn, principal, self.object).await?;
            let Body::Resource(fresh) = fresh.body else {
                return Ok(false);
            };
            if !opened.preserves_sources(&fresh) {
                return Ok(false);
            }
        }
        current_cut(conn, principal, self.cut.as_ref(), now).await
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_01cdf1c787, __api_install_01cdf1c787, __api_callback_01cdf1c787;
    slot __ApiCallback01cdf1c787, __API_CALLBACK_01CDF1C787, __api_set_01cdf1c787;
    base orch_api_base::routes::resources::projection::single;

    async fn current_cut<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<&'__api orch_api_base::routes::document_tree::metadata::Cut>, arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ec4d5a12fd, __api_install_ec4d5a12fd, __api_callback_ec4d5a12fd;
    slot __ApiCallbackec4d5a12fd, __API_CALLBACK_EC4D5A12FD, __api_set_ec4d5a12fd;
    base orch_api_base::routes::resources::projection::single;

    async fn bind_origins<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api mut orch_api_base::key_operations::admission::Prepared, arg2: &'static str, arg3: &'__api [orch_api_base::routes::resources::projection::single::Origin]) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_07ede148ea, __api_install_07ede148ea, __api_callback_07ede148ea;
    slot __ApiCallback07ede148ea, __API_CALLBACK_07EDE148EA, __api_set_07ede148ea;
    base orch_api_base::routes::resources::projection::single;

    fn resource(arg0: orch_api_base::routes::resources::projection::Rendered) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Resource>;
}

#[cfg(any(test, feature = "test-support"))]
#[path = "single/tests.rs"]
pub mod tests;
