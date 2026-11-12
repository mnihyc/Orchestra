//! Pause, enable and delete previews read the shared dependency and work service with native resource authority.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;
/// Pause notes remain a single visible line and an empty field means no note.
pub fn note(value: Option<&str>) -> ApiResult<Option<&str>> {
    if value.is_some_and(|value| value.contains(['\n', '\r', '\u{2028}', '\u{2029}'])) {
        return Err(ApiError::invalid("note", "must be one line"));
    }
    Ok(value.map(str::trim).filter(|value| !value.is_empty()))
}
