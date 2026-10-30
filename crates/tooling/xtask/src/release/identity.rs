//! Build identity for a component Cargo does not build: the frontend's, asked for by its build.
//!
//! A Rust program gets its identity from its build script. The browser
//! application is built by npm, so its build asks this command instead, and gets
//! the same answer a build script would: the release version, the tag if the
//! build is made from one, and the component's own build number, taken from the
//! same counters (`ORCH_BUILD_NUMBER_DIR` names another directory of them when
//! the tracked ones must stay as they are).

use crate::release::ReleaseError;
use serde_json::{Value, json};

/// Who a component's build is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The component: `frontend`, `server`, `connector` or `cli`.
    pub component: String,
    /// The release version.
    pub version: String,
    /// The release tag, when the build is made from one.
    pub tag: Option<String>,
    /// The component's build number.
    pub build_number: u64,
}

impl Identity {
    /// The identity as JSON, with the same members `orchctl version --json` has.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({
            "component": self.component,
            "version": self.version,
            "tag": self.tag.clone().unwrap_or_default(),
            "build_number": self.build_number.to_string(),
        })
    }

    /// The variables a build of the component is given, named as the Rust
    /// builds' are, so that a tool needs to know one way of asking.
    #[must_use]
    pub fn environment(&self) -> Vec<(String, String)> {
        vec![
            ("ORCH_RELEASE_VERSION".to_owned(), self.version.clone()),
            (
                "ORCH_RELEASE_TAG".to_owned(),
                self.tag.clone().unwrap_or_default(),
            ),
            (
                format!("ORCH_{}_BUILD_NUMBER", self.component.to_ascii_uppercase()),
                self.build_number.to_string(),
            ),
        ]
    }
}

/// Takes the identity of `component`'s next build. This advances the
/// component's counter, as a build of it would.
pub fn identity(component: &str) -> Result<Identity, ReleaseError> {
    let release = orch_build_support::release_identity(env!("CARGO_PKG_VERSION"))?;
    let build_number = orch_build_support::take_build_number(component)?;
    Ok(Identity {
        component: component.to_owned(),
        version: release.version,
        tag: release.tag,
        build_number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend() -> Identity {
        Identity {
            component: "frontend".to_owned(),
            version: "0.1.0".to_owned(),
            tag: Some("v0.1.0".to_owned()),
            build_number: 7,
        }
    }

    #[test]
    fn a_build_is_given_the_variables_a_rust_build_is_given() {
        let variables = frontend().environment();
        assert!(variables.contains(&("ORCH_RELEASE_VERSION".to_owned(), "0.1.0".to_owned())));
        assert!(variables.contains(&("ORCH_RELEASE_TAG".to_owned(), "v0.1.0".to_owned())));
        assert!(variables.contains(&("ORCH_FRONTEND_BUILD_NUMBER".to_owned(), "7".to_owned())));
    }

    #[test]
    fn a_build_that_is_made_from_no_tag_says_so_with_an_empty_one() {
        let mut untagged = frontend();
        untagged.tag = None;
        assert_eq!(untagged.json()["tag"], "");
    }
}
