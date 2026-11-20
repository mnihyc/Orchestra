//! Native absolute addresses and relative workspace paths use normalized separators.
//!
//! Relative paths start at the workspace; the empty path names its root. Lexical relative
//! traversal is refused before dispatch. Absolute paths are resolved by the selected machine
//! under the command's admitted reach, including link containment and native source rules.

orch_api_base::api_family_callback! {
    bind __api_bind_4f59b857ad, __api_install_4f59b857ad, __api_callback_4f59b857ad;
    slot __ApiCallback4f59b857ad, __API_CALLBACK_4F59B857AD, __api_set_4f59b857ad;
    base orch_api_base::routes::workspace::paths;

    fn clean<'__api>(arg0: &'__api str, arg1: &'__api str, arg2: bool) -> orch_api_base::middleware::requests::ApiResult<String>;
}

/// Recognizes POSIX, Windows drive and UNC roots without assuming the API server's OS.
#[must_use]
pub fn absolute(path: &str) -> bool {
    path.starts_with('/')
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && path.as_bytes()[2] == b'/')
}

/// How a path of a mount is written in the cache of file content, apart from the paths of the
/// workspace: a valid path never begins with `/`.
#[must_use]
pub fn cache_key(mount: Option<&str>, path: &str) -> String {
    match mount {
        Some(mount) => format!("/mount/{mount}/{path}"),
        None if absolute(path) => format!("/native/{path}"),
        None => path.to_owned(),
    }
}

/// The directory a path lies in: the path without its last component; the root for a path of one.
#[must_use]
pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| {
        if parent.is_empty() && path.starts_with('/') {
            "/"
        } else if parent.len() == 2 && parent.as_bytes()[1] == b':' {
            &path[..3]
        } else {
            parent
        }
    })
}
