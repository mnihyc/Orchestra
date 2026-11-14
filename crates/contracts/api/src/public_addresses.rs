//! The browser addresses of one installation, published together at one revision.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Primary and alternate HTTP origins. A write validates the entire value before storing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicAddresses {
    /// The address used in newly produced links, commands and deliveries.
    pub public_url: String,
    /// Other declared browser origins of the same installation.
    pub alternate_urls: Vec<String>,
}

/// A field that is not an HTTP origin. It never carries the rejected input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("must be an HTTP(S) origin without credentials, path, query or fragment")]
pub struct InvalidPublicAddress {
    /// The field a person can correct.
    pub field: &'static str,
}

impl PublicAddresses {
    /// Validate and canonicalize both fields as one value. Repeated origins are retained once.
    pub fn validated(&self) -> Result<Self, InvalidPublicAddress> {
        let public_url = origin(&self.public_url, "public_url")?;
        let mut seen = std::collections::BTreeSet::from([public_url.clone()]);
        let mut alternate_urls = Vec::with_capacity(self.alternate_urls.len());
        for address in &self.alternate_urls {
            let address = origin(address, "alternate_urls")?;
            if seen.insert(address.clone()) {
                alternate_urls.push(address);
            }
        }
        Ok(Self {
            public_url,
            alternate_urls,
        })
    }
}

fn origin(address: &str, field: &'static str) -> Result<String, InvalidPublicAddress> {
    let invalid = || InvalidPublicAddress { field };
    let uri: http::Uri = address.parse().map_err(|_| invalid())?;
    let scheme = uri
        .scheme_str()
        .filter(|s| matches!(*s, "http" | "https"))
        .ok_or_else(invalid)?;
    let authority = uri.authority().ok_or_else(invalid)?;
    let host = authority.host();
    let suffix = authority.as_str().strip_prefix(host).ok_or_else(invalid)?;
    if host.is_empty()
        || authority.as_str().contains('@')
        || !matches!(uri.path(), "" | "/")
        || uri.query().is_some()
        || address.contains(['#', '\\', '%'])
        || (!suffix.is_empty()
            && suffix
                .strip_prefix(':')
                .is_none_or(|port| port.parse::<u16>().is_err()))
        || authority.port_u16() == Some(0)
    {
        return Err(invalid());
    }
    let host = host.to_ascii_lowercase();
    let port = authority
        .port_u16()
        .filter(|port| !matches!((scheme, *port), ("http", 80) | ("https", 443)));
    Ok(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// The committed address revision. File configuration seeds it only when no row exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PublicAddressSnapshot {
    /// The effective primary and alternate addresses, read atomically.
    #[serde(flatten)]
    pub addresses: PublicAddresses,
    /// The monotone system-store revision used for compare-and-set publication.
    pub revision: i64,
    /// Whether the effective addresses are the persisted file seed rather than an override.
    pub from_file_seed: bool,
}
