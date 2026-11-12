//! The individual native whole, through-thread and retained Environment reading shapes.

pub use super::*;
pub enum Shape {
    Live {
        projection: PreparedInventory,
        reading: reading::Reading,
    },
    Retained(retained::PreparedRead),
}

pub struct Prepared {
    pub shape: Shape,
    pub disclosure: node_view::ReferenceDisclosure,
}

impl Prepared {
    pub async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Environment> {
        match &self.shape {
            Shape::Retained(row) => row.render(ctx, conn, principal).await,
            Shape::Live { projection, .. } => {
                let view = show_inventory_prepared(ctx, conn, principal, projection)
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)?;
                finish_shown(ctx, conn, principal, ctx.id("id")?, view).await
            }
        }
    }

    pub fn preserves(&self, fresh: &Self) -> bool {
        let same = match (&self.shape, &fresh.shape) {
            (Shape::Live { reading: old, .. }, Shape::Live { reading: now, .. }) => {
                *old == reading::Reading::ThroughAThread || *now == reading::Reading::Whole
            }
            (Shape::Retained(old), Shape::Retained(now)) => {
                old.id == now.id
                    && old.home == now.home
                    && old.deleted_at == now.deleted_at
                    && (!old.authority.0 || now.authority.0)
                    && (!old.authority.1 || now.authority.1)
            }
            _ => false,
        };
        same && (!self.whole() || self.projection().preserves_sources(fresh.projection()))
            && self.disclosure.names.is_subset(&fresh.disclosure.names)
            && self
                .disclosure
                .displays
                .is_subset(&fresh.disclosure.displays)
    }

    pub fn projection(&self) -> &PreparedInventory {
        match &self.shape {
            Shape::Live { projection, .. } => projection,
            Shape::Retained(row) => &row.projection,
        }
    }
    pub fn projection_mut(&mut self) -> &mut PreparedInventory {
        match &mut self.shape {
            Shape::Live { projection, .. } => projection,
            Shape::Retained(row) => &mut row.projection,
        }
    }
    pub fn whole(&self) -> bool {
        !matches!(
            &self.shape,
            Shape::Live {
                reading: reading::Reading::ThroughAThread,
                ..
            }
        )
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_07c8ba3b15, __api_install_07c8ba3b15, __api_callback_07c8ba3b15;
    slot __ApiCallback07c8ba3b15, __API_CALLBACK_07C8BA3B15, __api_set_07c8ba3b15;
    base orch_api_base::routes::environments::read_sources;

    async fn prepare<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: &'__api orch_api_contract::nodes::DeletedInspectionQuery, arg4: &'static orch_api_contract::operation_sources::Declaration<orch_api_contract::environment_sources::GetSource>) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::routes::environments::read_sources::Prepared, Vec<orch_engine::operation_scope::sources::Facts<orch_api_contract::environment_sources::GetSource>>)>;
}
