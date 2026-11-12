//! The existing UUID-ordered native choice windows, scoped before filling their native bound.

use super::*;

use futures::future::BoxFuture;
pub(super) trait Candidate: Send {
    fn resource(&self) -> &resources::Resource;
}

impl Candidate for resources::Seen {
    fn resource(&self) -> &resources::Resource {
        &self.resource
    }
}

impl Candidate for usable::Usables {
    fn resource(&self) -> &resources::Resource {
        &self.resource
    }
}

pub(super) trait Reader: Sync {
    type Row: Candidate;
    fn read<'a>(
        &'a self,
        conn: &'a mut Conn,
        page: Page,
    ) -> BoxFuture<'a, ApiResult<resources::Listing<Self::Row>>>;
}

pub(super) trait Bounds: Sync {
    fn allows<'a>(
        &'a self,
        conn: &'a mut Conn,
        rows: Vec<&'a resources::Resource>,
    ) -> BoxFuture<'a, ApiResult<Vec<bool>>>;
}

pub(super) struct Visible<'a> {
    pub principal: &'a Principal,
    pub kind: ResourceKind,
}

impl Reader for Visible<'_> {
    type Row = resources::Seen;
    fn read<'a>(
        &'a self,
        conn: &'a mut Conn,
        page: Page,
    ) -> BoxFuture<'a, ApiResult<resources::Listing<Self::Row>>> {
        Box::pin(async move {
            resources::visible(conn, self.principal, self.kind, page, Policy::default())
                .await
                .map_err(ApiError::internal)
        })
    }
}

pub(super) struct Usable<'a> {
    pub principal: &'a Principal,
    pub kind: ResourceKind,
    pub thread: Id,
    pub policies: &'a Policies,
    pub manage_environments: bool,
}

impl Reader for Usable<'_> {
    type Row = usable::Usables;
    fn read<'a>(
        &'a self,
        conn: &'a mut Conn,
        page: Page,
    ) -> BoxFuture<'a, ApiResult<resources::Listing<Self::Row>>> {
        Box::pin(async move {
            if self.manage_environments {
                usable::manageable_environments(
                    conn,
                    self.principal,
                    Some(self.thread),
                    self.policies,
                    page,
                )
                .await
                .map_err(ApiError::internal)
            } else {
                usable::usable_list(
                    conn,
                    self.principal,
                    self.kind,
                    Some(self.thread),
                    self.policies,
                    page,
                )
                .await
                .map_err(ApiError::internal)
            }
        })
    }
}

pub(super) async fn select<R: Reader>(
    conn: &mut Conn,
    reader: &R,
    bounds: Option<&dyn Bounds>,
) -> ApiResult<resources::Listing<R::Row>> {
    let Some(bounds) = bounds else {
        return reader
            .read(
                conn,
                Page {
                    after: None,
                    limit: LIST_LIMIT,
                },
            )
            .await;
    };
    let mut items = Vec::new();
    let mut after = None;
    loop {
        let candidates = reader
            .read(
                conn,
                Page {
                    after,
                    limit: LIST_LIMIT,
                },
            )
            .await?;
        let keep = bounds
            .allows(
                conn,
                candidates.items.iter().map(Candidate::resource).collect(),
            )
            .await?;
        if keep.len() != candidates.items.len() {
            return Err(ApiError::internal(
                "the native choice source omitted a candidate",
            ));
        }
        for (row, keep) in candidates.items.into_iter().zip(keep) {
            if keep {
                items.push(row);
            }
            if items.len() == LIST_LIMIT {
                return Ok(resources::Listing { items, next: None });
            }
        }
        let Some(next) = candidates.next else {
            return Ok(resources::Listing { items, next: None });
        };
        if after.is_some_and(|old| next <= old) {
            return Err(ApiError::internal(
                "the native choice cursor did not advance",
            ));
        }
        after = Some(next);
    }
}
