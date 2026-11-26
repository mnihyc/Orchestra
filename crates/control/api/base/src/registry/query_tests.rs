//! The actual HTTP query decoder keeps numeric paging through a flattened contract.

use super::read_query;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::storage::StorageGet;

use orch_api_contract::storage::StorageKind;

use orch_api_contract::storage::StorageQuery;

#[cfg(any(test, feature = "test-support"))]
fn public_request_text_is_repaired_without_changing_passwords_or_creation_keys()
-> crate::middleware::requests::ApiResult<()> {
    use orch_api_contract::auth::{AuthRegister, AuthRegisterBody};
    use orch_api_contract::threads::{ContentPart, ThreadInputCreate, ThreadInputCreateBody};
    let bytes=serde_json::json!({"username":"fixture","display_name":"Visible\0name","password":"synthetic\0password"}).to_string();
    let body: AuthRegisterBody = super::decode_body(&AuthRegister::ROUTE, bytes.as_bytes())?;
    assert_eq!(body.display_name, "Visible�name");
    assert_eq!(body.password, "synthetic\0password");
    use orch_api_contract::imports::{ImportRequest, NodeImportCreate};
    let bytes = serde_json::json!({"client_id":"import\0key","source":{"files":[]}}).to_string();
    let body: ImportRequest = super::decode_body(&NodeImportCreate::ROUTE, bytes.as_bytes())?;
    assert_eq!(body.client_id, "import\0key");

    let bytes=serde_json::json!({"content":[{"type":"text","text":"Public\0words"}],"delivery":"queue","client_id":"source\0key"}).to_string();
    let body: ThreadInputCreateBody =
        super::decode_body(&ThreadInputCreate::ROUTE, bytes.as_bytes())?;
    assert_eq!(body.client_id, "source\0key");
    assert!(matches!(&body.content[0],ContentPart::Text {text} if text=="Public�words"));
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_public_request_text_is_repaired_without_changing_passwords_or_creation_keys()
-> crate::middleware::requests::ApiResult<()> {
    public_request_text_is_repaired_without_changing_passwords_or_creation_keys()
}
#[cfg(any(test, feature = "test-support"))]
fn storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
-> crate::middleware::requests::ApiResult<()> {
    let query: StorageQuery = read_query(
        &StorageGet::ROUTE,
        Some("kind=image&q=kept&limit=501&cursor=next"),
    )?;
    assert_eq!(query.kind, Some(StorageKind::Image));
    assert_eq!(query.page.q.as_deref(), Some("kept"));
    assert_eq!(query.page.limit, Some(501));
    assert_eq!(query.page.cursor.as_deref(), Some("next"));

    for invalid in ["limit=-1", "limit=4294967296", "limit=1.5", "limit="] {
        assert!(matches!(
            read_query::<StorageQuery>(&StorageGet::ROUTE, Some(invalid)),
            Err(error) if error.code() == ErrorCode::RequestInvalid
        ));
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
-> crate::middleware::requests::ApiResult<()> {
    self::storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
}
