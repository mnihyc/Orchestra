//! A path of a workspace as a request writes it, and what it may be.
//!
//! A path is relative to the root of the environment's workspace and `/`-separated: no leading `/`,
//! no empty, `.` or `..` components, at most 4 096 bytes; the empty string is the root. A path that
//! leaves the workspace does not exist for anybody, and the request is answered as if there were
//! nothing there without the machine being asked. Windows separators are converted. What the
//! workspace's ignore rules leave out is the machine's to say, since the rules are applied where
//! the files are.

use crate::middleware::requests::{ApiError, ApiResult};

/// The longest path a workspace holds, in bytes.
pub const PATH_MAX_BYTES: usize = 4_096;

/// The path `raw` as the workspace keeps it. `allow_root` says whether the empty path, the root of
/// the workspace, is a path here.
pub fn clean(field: &str, raw: &str, allow_root: bool) -> ApiResult<String> {
    let converted = raw.replace('\\', "/");
    let trimmed = converted.strip_suffix('/').unwrap_or(&converted);
    if trimmed.starts_with('/') {
        return Err(ApiError::not_found());
    }
    if trimmed.len() > PATH_MAX_BYTES || trimmed.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            field,
            "a path has at most 4096 bytes and no control characters",
        ));
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

/// How a path of a mount is written in the cache of file content, apart from the paths of the
/// workspace: a valid path never begins with `/`.
#[must_use]
pub fn cache_key(mount: Option<&str>, path: &str) -> String {
    match mount {
        Some(mount) => format!("/mount/{mount}/{path}"),
        None => path.to_owned(),
    }
}

/// The directory a path lies in: the path without its last component; the root for a path of one.
#[must_use]
pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// The last component of a path.
#[must_use]
pub fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::common::ErrorCode;

    #[test]
    fn a_path_is_relative_and_does_not_climb() {
        assert_eq!(clean("path", "src/main.rs", false).unwrap(), "src/main.rs");
        assert_eq!(clean("path", "src\\lib.rs", false).unwrap(), "src/lib.rs");
        assert_eq!(clean("path", "src/", false).unwrap(), "src");
        assert_eq!(clean("path", "", true).unwrap(), "");
        assert_eq!(
            clean("path", "", false).unwrap_err().code(),
            ErrorCode::RequestInvalid
        );
        assert_eq!(
            clean("path", "/etc/passwd", true).unwrap_err().code(),
            ErrorCode::NotFound
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

    #[test]
    fn a_path_has_a_parent_and_a_name() {
        assert_eq!(parent_of("a/b/c"), "a/b");
        assert_eq!(parent_of("c"), "");
        assert_eq!(name_of("a/b/c"), "c");
        assert_eq!(cache_key(None, "a/b"), "a/b");
        assert_eq!(cache_key(Some("data"), "a/b"), "/mount/data/a/b");
    }
}
