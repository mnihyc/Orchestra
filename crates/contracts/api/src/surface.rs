//! HTTP transport selection. This does not grant an operation or alter its ordinary authority.

use crate::operations::{Delegation, for_route};
use crate::routes::Route;

/// Existing browser session access-token prefix.
pub const BROWSER_TOKEN_PREFIX: &str = "oat_";
/// Existing personal/service API-key prefix.
pub const KEY_TOKEN_PREFIX: &str = "opat_";

/// The HTTP surface fixed by the server's route mount, never supplied by a request header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpSurface {
    /// Browser sessions and the existing public authentication/continuation protocols.
    Browser,
    /// Account API keys and their origin-bound protocol continuations.
    Key,
}

impl HttpSurface {
    /// The versioned root of this HTTP surface.
    #[must_use]
    pub const fn base_path(self) -> &'static str {
        match self {
            Self::Browser => crate::common::BROWSER_BASE_PATH,
            Self::Key => crate::common::KEY_BASE_PATH,
        }
    }

    /// Client-side transport selection only; authentication still resolves the actual credential.
    #[must_use]
    pub fn for_token(token: &str) -> Option<Self> {
        if token.starts_with(BROWSER_TOKEN_PREFIX) {
            Some(Self::Browser)
        } else if token.starts_with(KEY_TOKEN_PREFIX) {
            Some(Self::Key)
        } else {
            None
        }
    }

    /// Placement of an already declared endpoint, without deriving any privilege from its URL.
    #[must_use]
    pub fn serves(self, route: &Route) -> bool {
        match self {
            Self::Browser => route.name != "OpenapiGet",
            Self::Key => {
                for_route(route).is_ok_and(|operation| operation.delegation == Delegation::Business)
                    || continuation(route)
            }
        }
    }
}

/// Existing ticket protocols shared by origin-specific HTTP mounts, not extra business operations.
#[must_use]
pub fn continuation(route: &Route) -> bool {
    matches!(
        route.name,
        "TerminalAttach"
            | "AgentTerminalAttach"
            | "BrowserAttach"
            | "DisplayAttach"
            | "PortUseSocket"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_keeps_every_business_operation_including_session_annotations() {
        for route in crate::routes::all() {
            let operation = for_route(&route).unwrap();
            match operation.delegation {
                Delegation::Business => assert!(HttpSurface::Key.serves(&route), "{}", route.name),
                Delegation::Public | Delegation::BrowserElevation => {
                    assert!(!HttpSurface::Key.serves(&route), "{}", route.name)
                }
                Delegation::AuthenticationProtocol => assert_eq!(
                    HttpSurface::Key.serves(&route),
                    continuation(&route),
                    "{}",
                    route.name
                ),
            }
            assert_eq!(
                HttpSurface::Browser.serves(&route),
                route.name != "OpenapiGet"
            );
        }
    }
}
