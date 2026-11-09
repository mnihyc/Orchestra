//! The actual HTTP query decoder keeps numeric paging through a flattened contract.
use super::read_query;
use orch_api_contract::{
    common::ErrorCode,
    routes::Endpoint,
    storage::{StorageGet, StorageKind, StorageQuery},
};

#[test]
fn storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
-> crate::middleware::requests::ApiResult<()> {
    let query: StorageQuery = read_query(&StorageGet::ROUTE, Some("kind=image&limit=1"))?;
    assert_eq!(query.kind, Some(StorageKind::Image));
    assert_eq!(query.page.limit, Some(1));
    assert_eq!(query.page.limit(), 1);
    assert!(query.page.cursor.is_none());

    for invalid in ["limit=-1", "limit=4294967296", "limit=1.5", "limit="] {
        assert!(matches!(
            read_query::<StorageQuery>(&StorageGet::ROUTE, Some(invalid)),
            Err(error) if error.code() == ErrorCode::RequestInvalid
        ));
    }
    Ok(())
}
