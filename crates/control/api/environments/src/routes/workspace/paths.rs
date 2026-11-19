//! Native absolute addresses and relative attached-root paths; the machine proves containment.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;
/// The longest path a workspace holds, in bytes.
pub const PATH_MAX_BYTES: usize = 4_096;

/// The path `raw` as the workspace keeps it. `allow_root` says whether the empty path, the root of
/// the workspace, is a path here.
pub fn clean(field: &str, raw: &str, allow_root: bool) -> ApiResult<String> {
    let converted = raw.replace('\\', "/");
    let native = absolute(&converted);
    let root = converted == "/"
        || (converted.len() == 3 && converted.as_bytes()[1] == b':' && converted.ends_with('/'));
    let trimmed = if root {
        converted.as_str()
    } else {
        converted.trim_end_matches('/')
    };
    if trimmed.len() > PATH_MAX_BYTES || trimmed.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            field,
            "a path has at most 4096 bytes and no control characters",
        ));
    }
    if native {
        // Dot components, drive and link semantics belong to the selected environment's native resolver.
        return Ok(trimmed.to_owned());
    }
    if trimmed.is_empty() {
        return if allow_root {
            Ok(String::new())
        } else {
            Err(ApiError::invalid(
                field,
                "this names a file or a directory, not the root",
            ))
        };
    }
    for part in trimmed.split('/') {
        match part {
            ".." => return Err(ApiError::not_found()),
            "" | "." => {
                return Err(ApiError::invalid(
                    field,
                    "a path has no empty component and no `.`",
                ));
            }
            _ => {}
        }
    }
    Ok(trimmed.to_owned())
}
orch_api_base::__api_bind_4f59b857ad!();

pub use orch_api_base::routes::workspace::paths::{absolute, cache_key, parent_of};

/// The name of a mount as a request writes it: a word, as the machine knows it.
pub fn mount(field: &str, raw: Option<&str>) -> ApiResult<Option<String>> {
    match raw {
        None => Ok(None),
        Some(name) => {
            let name = name.trim();
            if name.is_empty() || name.contains('/') || name.chars().any(char::is_control) {
                return Err(ApiError::invalid(field, "this is the name of a mount"));
            }
            Ok(Some(name.to_owned()))
        }
    }
}

/// The last component of a path.
#[must_use]
pub fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_api_contract::common::ErrorCode;
    #[cfg(any(test, feature = "test-support"))]
    fn a_path_is_relative_and_does_not_climb() {
        assert_eq!(clean("path", "src/main.rs", false).unwrap(), "src/main.rs");
        assert_eq!(clean("path", "src\\lib.rs", false).unwrap(), "src/lib.rs");
        assert_eq!(clean("path", "src/", false).unwrap(), "src");
        assert_eq!(clean("path", "", true).unwrap(), "");
        assert_eq!(
            clean("path", "", false).unwrap_err().code(),
            ErrorCode::RequestInvalid
        );
        assert_eq!(clean("path", "/etc/passwd", true).unwrap(), "/etc/passwd");
        assert_eq!(clean("path", "/", true).unwrap(), "/");
        assert_eq!(clean("path", "C:/", true).unwrap(), "C:/");
        assert_eq!(
            clean("path", "/tmp/../var/file", true).unwrap(),
            "/tmp/../var/file"
        );
        assert_eq!(
            clean("path", "a/../../b", true).unwrap_err().code(),
            ErrorCode::NotFound
        );
        assert_eq!(
            clean("path", "a//b", true).unwrap_err().code(),
            ErrorCode::RequestInvalid
        );
        assert_eq!(
            clean("path", "a/./b", true).unwrap_err().code(),
            ErrorCode::RequestInvalid
        );
        assert_eq!(
            clean("path", "a\nb", true).unwrap_err().code(),
            ErrorCode::RequestInvalid
        );
        assert_eq!(
            clean("path", &"x".repeat(PATH_MAX_BYTES + 1), true)
                .unwrap_err()
                .code(),
            ErrorCode::RequestInvalid
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_path_is_relative_and_does_not_climb() {
        self::a_path_is_relative_and_does_not_climb()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_path_has_a_parent_and_a_name() {
        assert_eq!(parent_of("a/b/c"), "a/b");
        assert_eq!(parent_of("c"), "");
        assert_eq!(parent_of("/file"), "/");
        assert_eq!(parent_of("C:/file"), "C:/");
        assert_ne!(
            cache_key(None, "/mount/data/a/b"),
            cache_key(Some("data"), "a/b")
        );
        assert_eq!(name_of("a/b/c"), "c");
        assert_eq!(cache_key(None, "a/b"), "a/b");
        assert_eq!(cache_key(Some("data"), "a/b"), "/mount/data/a/b");
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_path_has_a_parent_and_a_name() {
        self::a_path_has_a_parent_and_a_name()
    }
}
