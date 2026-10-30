//! Target resolution and configured network restrictions.
//!
//! What an org or a person set up must not be a way to reach the machines
//! behind the system. A webhook's address is looked at in three ways: it has
//! to be a web address (`http` or `https`, with a host); its host is resolved
//! here, and a host that resolves to anything but a public address
//! is refused unless the list of hosts a system admin allowed names it; and
//! the connection goes to an address that was looked at, so that a name that
//! answers differently the second time leads nowhere new. A redirect is another
//! address and is looked at in the same way, again, before it is followed.
//!
//! The system's own channels are not held to the list.

use orch_notifier_contract::delivery::DeliverError;
use orch_notifier_contract::outbound;
use std::net::{IpAddr, SocketAddr};
use url::{Host, Url};

/// The addresses of a webhook's host that may be connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The address, as a web address.
    pub url: Url,
    /// The host, as the connection names it for the certificate.
    pub host: String,
    /// The addresses to connect to, each looked at.
    pub addresses: Vec<SocketAddr>,
}

/// Reads an address as a webhook's: `http` or `https`, with a host.
pub fn parse(text: &str) -> Result<Url, DeliverError> {
    let url = Url::parse(text.trim()).map_err(|error| {
        DeliverError::refused(
            "invalid_url",
            &format!("the address is not a web address: {error}"),
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(DeliverError::refused(
            "invalid_url",
            "the address starts with http:// or https://",
        ));
    }
    if url.host().is_none() {
        return Err(DeliverError::refused(
            "invalid_url",
            "the address names no host",
        ));
    }
    Ok(url)
}

/// Resolves the host of `url` and keeps the addresses `allowed` lets a
/// connection go to: all of them where it is `None` (the system's own
/// channels), and otherwise those that are not on a loopback, link-local or
/// private network, or that the list names, by host or by address.
pub async fn resolve(url: &Url, allowed: Option<&[String]>) -> Result<Target, DeliverError> {
    let host = url
        .host()
        .ok_or_else(|| DeliverError::refused("invalid_url", "the address names no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| DeliverError::refused("invalid_url", "the address names no port"))?;
    let (name, found): (String, Vec<SocketAddr>) = match host {
        Host::Ipv4(address) => (
            address.to_string(),
            vec![SocketAddr::new(IpAddr::V4(address), port)],
        ),
        Host::Ipv6(address) => (
            address.to_string(),
            vec![SocketAddr::new(IpAddr::V6(address), port)],
        ),
        Host::Domain(domain) => {
            let looked_up = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|error| {
                    DeliverError::temporary(
                        "unreachable",
                        &format!("{domain} cannot be looked up: {error}"),
                    )
                })?;
            (domain.to_owned(), looked_up.collect())
        }
    };
    if found.is_empty() {
        return Err(DeliverError::temporary(
            "unreachable",
            &format!("{name} has no address"),
        ));
    }
    let mut addresses = Vec::new();
    let mut refusal = None;
    for address in found {
        match outbound::may_connect(&name, address.ip(), allowed) {
            Ok(()) => addresses.push(address),
            Err(refused) => refusal = Some(refused),
        }
    }
    match (addresses.is_empty(), refusal) {
        (true, Some(refused)) => Err(DeliverError::refused(
            "address_not_allowed",
            &refused.to_string(),
        )),
        _ => Ok(Target {
            url: url.clone(),
            host: name,
            addresses,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_web_address_with_a_host_is_a_webhook() {
        assert!(parse("https://hooks.example/x?y=1").is_ok());
        assert!(parse(" http://10.0.0.1:8080/x ").is_ok());
        for bad in [
            "ftp://x.example/",
            "file:///etc/passwd",
            "hooks.example",
            "https://",
            "javascript:alert(1)",
            "",
        ] {
            assert_eq!(parse(bad).unwrap_err().code, "invalid_url", "{bad}");
        }
    }

    #[tokio::test]
    async fn an_address_on_a_private_network_is_refused_unless_the_list_names_it() {
        let url = parse("http://127.0.0.1:8080/hook").unwrap();
        let held = resolve(&url, Some(&[])).await.unwrap_err();
        assert_eq!(
            (held.code.as_str(), held.retryable),
            ("address_not_allowed", false)
        );
        assert!(held.message.contains("127.0.0.1"));
        assert!(resolve(&url, Some(&["127.0.0.1".to_owned()])).await.is_ok());
        assert!(
            resolve(&url, Some(&["127.0.0.0/8".to_owned()]))
                .await
                .is_ok()
        );
        assert!(
            resolve(&url, Some(&["10.0.0.0/8".to_owned()]))
                .await
                .is_err()
        );
        assert!(
            resolve(&url, None).await.is_ok(),
            "the system's own channels are not held to the list"
        );
        for private in [
            "http://[::1]/x",
            "http://192.168.1.5/x",
            "http://169.254.169.254/latest",
            "http://0.0.0.0/x",
        ] {
            assert!(
                resolve(&parse(private).unwrap(), Some(&[])).await.is_err(),
                "{private}"
            );
        }
        assert!(
            resolve(&parse("http://93.184.216.34/x").unwrap(), Some(&[]))
                .await
                .is_ok(),
            "a public address needs no listing"
        );
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_a_private_address_is_held_to_the_list_too() {
        let url = parse("http://localhost:9/x").unwrap();
        assert!(resolve(&url, Some(&[])).await.is_err());
        let target = resolve(&url, Some(&["localhost".to_owned()]))
            .await
            .unwrap();
        assert_eq!(target.host, "localhost");
        assert!(target.addresses.iter().all(|address| address.port() == 9));
    }
}
