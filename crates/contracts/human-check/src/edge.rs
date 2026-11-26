//! What a web edge's content policy lets a page reach, as far as a human check needs: the one reading of a policy
//! that the API (before it lets the check be switched on, and while it is on) and the doctor share.
//!
//! A vendor's widget works only where the browser may load its script and frames and let it speak back, so a
//! policy that does not name the vendor's hosts blocks the widget for everybody, and a check nobody can pass shuts
//! the form. [`lacking`] says what a policy leaves out. It reads the policies a browser would enforce together (a
//! page may arrive with several, from the edge and from a front of the operator's own, and each must allow what is
//! loaded) and is careful in one direction: a host it cannot be sure a policy allows is reported as lacking, never
//! the other way, so that a refusal can be tried again after the policy is set, and no check goes on that cannot load.

use std::time::Duration;

use async_trait::async_trait;

use crate::{EdgeHosts, Unreachable};

/// One host that a policy has to allow for a directive, and does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lack {
    /// The directive that has to allow it.
    pub directive: &'static str,
    /// The source expression it has to name, as the policy would write it.
    pub source: &'static str,
}

impl std::fmt::Display for Lack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} in {}", self.source, self.directive)
    }
}

/// What the web edge answered when its page was asked for: the status, and every `Content-Security-Policy` header
/// the answer carried, as written (a header may hold several policies, separated by commas).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeAnswer {
    /// The HTTP status.
    pub status: u16,
    /// The value of each `Content-Security-Policy` header.
    pub policies: Vec<String>,
}

/// A way to ask the web edge for its page and read its content policy: the seam through which the API looks at its
/// own edge, so that its tests are scripted answers and need no network.
#[async_trait]
pub trait EdgeReader: Send + Sync + 'static {
    /// Asks `url` for its page, giving up after `timeout`. A failure to get an answer is [`Unreachable`].
    async fn read(&self, url: &str, timeout: Duration) -> Result<EdgeAnswer, Unreachable>;
}

/// The directives that decide whether a page may load from a host, nearest first: a directive that a policy does not
/// name falls back to the next, and the last is `default-src`.
fn fallbacks(directive: &str) -> &'static [&'static str] {
    match directive {
        "script-src" => &["script-src-elem", "script-src", "default-src"],
        "style-src" => &["style-src-elem", "style-src", "default-src"],
        "frame-src" => &["frame-src", "child-src", "default-src"],
        _ => &["connect-src", "default-src"],
    }
}

/// The sources the policy `policy` names for `directive`, after the fallbacks; none where it restricts nothing for it.
fn sources_of<'a>(policy: &'a str, directive: &str) -> Option<Vec<&'a str>> {
    let named = |name: &str| {
        policy.split(';').map(str::trim).find_map(|part| {
            let mut words = part.split_whitespace();
            words
                .next()
                .filter(|first| first.eq_ignore_ascii_case(name))
                .map(|_| words.collect::<Vec<_>>())
        })
    };
    fallbacks(directive).iter().find_map(|name| named(name))
}

/// A source expression taken apart: `[scheme://]host[:port][/path]`.
#[derive(Debug, PartialEq, Eq)]
struct Parts<'a> {
    scheme: Option<&'a str>,
    host: &'a str,
    port: Option<&'a str>,
    path: &'a str,
}

fn parts(source: &str) -> Option<Parts<'_>> {
    let (scheme, rest) = match source.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, source),
    };
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => (host, Some(port)),
        _ => (authority, None),
    };
    if host.is_empty() || host.starts_with('\'') {
        return None;
    }
    Some(Parts {
        scheme,
        host,
        port,
        path,
    })
}

/// Whether the allowed expression `allowed` lets a page reach everything the needed expression `needed` names: the
/// same host or a wildcard above it, on a scheme and port the page uses, and a path that is at least as wide.
fn covers(allowed: &str, needed: &str) -> bool {
    let allowed = allowed.trim();
    if allowed.eq_ignore_ascii_case("'none'") || allowed.starts_with('\'') {
        return false;
    }
    // Everything on the network, and everything on a scheme.
    if allowed == "*" || allowed.eq_ignore_ascii_case("https:") {
        return true;
    }
    let (Some(have), Some(need)) = (parts(allowed), parts(needed)) else {
        return false;
    };
    // A scheme-less source takes the scheme of the page, which is served over HTTPS.
    if have.scheme.is_some_and(|scheme| {
        !scheme.eq_ignore_ascii_case("https")
            && !need.scheme.is_some_and(|n| n.eq_ignore_ascii_case(scheme))
    }) {
        return false;
    }
    let have_host = have.host.to_ascii_lowercase();
    let need_host = need.host.to_ascii_lowercase();
    let host_ok = have_host == "*"
        || have_host == need_host
        || have_host.strip_prefix("*.").is_some_and(|tail| {
            need_host.ends_with(&format!(".{tail}")) || need_host == format!("*.{tail}")
        });
    // No port names the scheme's own; the services are reached on it.
    let port_ok = match have.port {
        None => need.port.is_none(),
        Some("*") => true,
        Some(port) => need.port == Some(port) || (port == "443" && need.port.is_none()),
    };
    let path_ok = if have.path.is_empty() || have.path == "/" {
        true
    } else if have.path.ends_with('/') {
        need.path.starts_with(have.path)
    } else {
        need.path == have.path
    };
    host_ok && port_ok && path_ok
}

/// Splits the values of the `Content-Security-Policy` headers into the policies a browser enforces.
fn enforced(values: &[String]) -> impl Iterator<Item = &str> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|policy| !policy.is_empty())
}

/// What the policies leave out of what a human check's widget needs. Empty where they allow all of it, or where
/// there is no policy at all.
#[must_use]
pub fn lacking(policies: &[String], hosts: &EdgeHosts) -> Vec<Lack> {
    let mut missing: Vec<Lack> = Vec::new();
    for (directive, needed) in [
        ("script-src", hosts.script),
        ("frame-src", hosts.frame),
        ("connect-src", hosts.connect),
        ("style-src", hosts.style),
    ] {
        for need in needed {
            let blocked = enforced(policies).any(|policy| {
                sources_of(policy, directive)
                    .is_some_and(|allowed| !allowed.iter().any(|source| covers(source, need)))
            });
            let lack = Lack {
                directive,
                source: need,
            };
            if blocked && !missing.contains(&lack) {
                missing.push(lack);
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    const HCAPTCHA: EdgeHosts = EdgeHosts {
        script: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
        frame: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
        connect: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
        style: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
    };
    const RECAPTCHA: EdgeHosts = EdgeHosts {
        script: &[
            "https://www.google.com/recaptcha/",
            "https://www.gstatic.com/recaptcha/",
        ],
        frame: &[
            "https://www.google.com/recaptcha/",
            "https://recaptcha.google.com/recaptcha/",
        ],
        connect: &["https://www.google.com/recaptcha/"],
        style: &[],
    };

    /// The policy the edge sent before a human check was a choice.
    const SHIPPED: &str = "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'";

    fn one(policy: &str) -> Vec<String> {
        vec![policy.to_owned()]
    }

    #[test]
    fn the_policy_the_edge_always_sent_has_no_room_for_either_service() {
        let hcaptcha = lacking(&one(SHIPPED), &HCAPTCHA);
        assert_eq!(hcaptcha.len(), 8, "{hcaptcha:?}");
        let recaptcha = lacking(&one(SHIPPED), &RECAPTCHA);
        assert_eq!(recaptcha.len(), 5, "{recaptcha:?}");
        assert_eq!(
            hcaptcha[0].to_string(),
            "https://hcaptcha.com in script-src"
        );
    }

    #[test]
    fn a_policy_with_the_hosts_written_out_lacks_nothing() {
        let policy = "default-src 'self'; script-src 'self' https://hcaptcha.com https://*.hcaptcha.com; style-src 'self' https://hcaptcha.com https://*.hcaptcha.com; connect-src 'self' https://hcaptcha.com https://*.hcaptcha.com; frame-src 'self' https://hcaptcha.com https://*.hcaptcha.com";
        assert!(lacking(&one(policy), &HCAPTCHA).is_empty());
    }

    #[test]
    fn a_wildcard_scheme_or_star_in_the_policy_of_the_operator_is_enough() {
        for allowed in ["https://*.hcaptcha.com https://hcaptcha.com", "https:", "*"] {
            let policy = format!(
                "default-src 'self'; script-src 'self' {allowed}; style-src {allowed}; connect-src {allowed}; frame-src {allowed}"
            );
            assert!(lacking(&one(&policy), &HCAPTCHA).is_empty(), "{allowed}");
        }
        // The apex is not a subdomain: the wildcard alone leaves it out.
        let policy = "default-src 'self'; script-src 'self' https://*.hcaptcha.com; style-src https://*.hcaptcha.com; connect-src https://*.hcaptcha.com; frame-src https://*.hcaptcha.com";
        let missing = lacking(&one(policy), &HCAPTCHA);
        assert_eq!(missing.len(), 4, "{missing:?}");
        assert!(
            missing
                .iter()
                .all(|lack| lack.source == "https://hcaptcha.com")
        );
    }

    #[test]
    fn a_directive_the_policy_does_not_name_falls_back_to_the_default() {
        // Frames fall back to child-src, then default-src; no default-src restricts nothing.
        let policy = "child-src https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/; script-src https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/; connect-src https://www.google.com";
        assert!(lacking(&one(policy), &RECAPTCHA).is_empty());
        assert!(lacking(&one("img-src 'self'"), &RECAPTCHA).is_empty());
        assert!(lacking(&[], &RECAPTCHA).is_empty());
    }

    #[test]
    fn a_path_must_be_at_least_as_wide_as_the_one_a_service_uses() {
        let narrow = "default-src 'self'; script-src https://www.google.com/recaptcha/api.js https://www.gstatic.com/recaptcha/; frame-src https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/; connect-src https://www.google.com/recaptcha/";
        let missing = lacking(&one(narrow), &RECAPTCHA);
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert_eq!(missing[0].directive, "script-src");
        let host_only = "default-src 'self'; script-src https://www.google.com https://www.gstatic.com; frame-src https://www.google.com https://recaptcha.google.com; connect-src https://www.google.com";
        assert!(lacking(&one(host_only), &RECAPTCHA).is_empty());
    }

    #[test]
    fn every_policy_that_arrives_must_allow_it() {
        let open = "default-src 'self'; script-src https:; style-src https:; connect-src https:; frame-src https:";
        let both = vec![open.to_owned(), SHIPPED.to_owned()];
        assert!(!lacking(&both, &HCAPTCHA).is_empty());
        // Several policies in one header are the same as several headers.
        assert_eq!(
            lacking(&[format!("{open}, {SHIPPED}")], &HCAPTCHA),
            lacking(&both, &HCAPTCHA)
        );
    }

    #[test]
    fn none_and_the_keywords_allow_nothing() {
        let policy = "default-src 'none'; script-src 'self' 'unsafe-inline' 'nonce-abc'";
        assert!(!lacking(&one(policy), &HCAPTCHA).is_empty());
    }
}
