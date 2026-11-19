//! Pause, enable and delete previews read the shared dependency and work service with native resource authority.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Registry;
use orch_api_contract::resource_facts::{self as api, FactTarget};

/// Binds the shared current-consequence read.
pub fn mount(registry: &mut Registry) {
    registry.on_snapshot_body_read_with_optional_operation(
        api::ResourceImpactRead,
        crate::key_operations::native::selected_body::<super::figures::Impact, (), FactTarget>,
        crate::key_operations::native::selected_body_read::<super::figures::Impact, (), FactTarget>,
    );
}

/// Pause notes remain a single visible line and an empty field means no note.
pub(crate) fn note(value: Option<&str>) -> ApiResult<Option<&str>> {
    if value.is_some_and(|value| value.contains(['\n', '\r', '\u{2028}', '\u{2029}'])) {
        return Err(ApiError::invalid("note", "must be one line"));
    }
    Ok(value.map(str::trim).filter(|value| !value.is_empty()))
}
