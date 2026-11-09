//! Native complete document metadata, selected without borrowing an endpoint's admission.

use crate::middleware::requests::ApiResult;
use crate::routes::access::page::{NameBounds, NameSources};
use orch_authz::{Policy, Principal};
use orch_db::{Conn, queries::documents::DocumentRow};
use orch_types::{Id, Time};

pub(crate) struct Prepared {
    row: DocumentRow,
    listed: orch_authz::documents::Listed,
    bounds: NameBounds,
}
impl Prepared {
    pub(crate) fn node(&self) -> Id {
        self.row.node_id
    }
    pub(crate) async fn names(&self, conn: &mut Conn) -> ApiResult<NameSources> {
        super::build::document_name_sources(conn, &self.row, &self.listed).await
    }
    pub(crate) fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
    pub(crate) async fn render(
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
