//! Native complete document metadata, selected without borrowing an endpoint's admission.

pub use crate::middleware::requests::ApiResult;

pub use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

use orch_db::queries::documents::DocumentRow;

pub use orch_types::Id;

use orch_types::Time;
pub struct Prepared {
    pub row: DocumentRow,
    pub listed: orch_authz::documents::Listed,
    pub bounds: NameBounds,
}

impl Prepared {
    pub fn node(&self) -> Id {
        self.row.node_id
    }
    pub async fn names(&self, conn: &mut Conn) -> ApiResult<NameSources> {
        super::build::document_name_sources(conn, &self.row, &self.listed).await
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
    pub async fn render(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        now: Time,
    ) -> ApiResult<orch_api_contract::documents::Document> {
        let labels: Vec<_> = self
            .bounds
            .nodes
            .iter()
            .copied()
            .map(super::build::WrittenLabelSource::Node)
            .collect();
        super::build::document_bounded(
            conn,
            principal,
            &self.row,
            &self.listed,
            now.get(),
            Policy::default(),
            super::build::DocumentLabels {
                names: Some(&self.bounds),
                sources: self.bounds.limited.then_some(labels.as_slice()),
            },
        )
        .await
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_205f3cab3b, __api_install_205f3cab3b, __api_callback_205f3cab3b;
    slot __ApiCallback205f3cab3b, __API_CALLBACK_205F3CAB3B, __api_set_205f3cab3b;
    base orch_api_base::routes::documents::metadata_projection;

    async fn prepare<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::documents::metadata_projection::Prepared>;
}
