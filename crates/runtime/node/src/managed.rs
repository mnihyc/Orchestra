//! Connector launch inputs and single-use enrollment.
//!
//! A unit's main process is the connector, started by the driver with the inputs the system gave in the
//! request: the roles, the address of the gateway, the certificate authority to believe, the tools, and a
//! **single-use enrollment token**. The connector enrolls with it at its first start and never again; its
//! identity is then in its data directory on the unit's state volume, so a restarted or recreated unit keeps
//! it. A unit that is replaced is a new machine to enroll, and the system gives it a token of its own.
//!
//! The token reaches the node sealed to the node's key ([`orch_connector_core::roles::Keys`]), because the
//! system stores no secret in a command. The node opens it, once, for the request it came with, and gives it
//! to the driver in the one place a driver needs it. Before it does, the node checks what the connector would
//! otherwise find out too late, inside a unit that then fails: that the token is a token, that the authority
//! given is the one it pins, that the gateway is an address the connector can enroll at, and that the tools
//! the unit is to have are in the cache. And it keeps a token from being given to two units: only its hash is
//! remembered, to refuse the second.

use orch_connector_core::roles::Keys;
use orch_driver_contract::runtime::{RuntimeSpec, SpecFault};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_link_contract::enrollment::EnrollmentToken;
use orch_pki::certificates::Certificate;
use orch_types::{Hash, Id, Json};

use crate::drivers::INVALID;
use crate::tools::ToolCache;

/// The schemes a unit's connector can enroll at: both are TLS. A token sent in the clear is a token anyone on
/// the way can spend.
const GATEWAY_SCHEMES: [&str; 2] = ["wss", "https"];

fn invalid(detail: impl Into<String>) -> CommandError {
    CommandError {
        code: INVALID.to_owned(),
        detail: detail.into(),
        retryable: false,
        details: None,
    }
}

/// Reads the request out of the body of `node.runtime.create`, opening the enrollment token that was sealed
/// to this machine for the command `command`.
///
/// A token that arrives as plain text is refused, and so is one that is still a reference: the first would
/// mean a command that holds a secret, the second a command that was sent as it is stored.
pub fn unseal(keys: &Keys, command: Id, mut body: Json) -> Result<RuntimeSpec, CommandError> {
    let token = body
        .get_mut("enroll_token")
        .ok_or_else(|| invalid("the request has no enrollment token"))?;
    let sealed = match token {
        Json::Object(members) if members.len() == 1 => match members.get("$sealed") {
            Some(Json::String(sealed)) => sealed.clone(),
            _ if members.contains_key("$secret") => {
                return Err(invalid(
                    "the enrollment token is still a reference: the system seals it when it sends the command",
                ));
            }
            _ => return Err(invalid("the enrollment token is not sealed")),
        },
        _ => {
            return Err(invalid(
                "the enrollment token must arrive sealed to this machine, and not as text",
            ));
        }
    };
    *token = Json::String(keys.open(command, &sealed)?);
    serde_json::from_value(body)
        .map_err(|error| invalid(format!("the request is not a runtime spec: {error}")))
}

/// What checking a request found out about its token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked {
    /// The hash of the enrollment token: the only trace of it the node keeps.
    pub token: Hash,
}

/// Checks a request for what the connector in its unit needs, and answers what the node keeps of it.
///
/// The name of the unit is not held to the one the runtime's identifier gives: a driver finds and touches
/// its units by their labels, and a name that differs does no harm.
pub fn check(spec: &RuntimeSpec, tools: &ToolCache) -> Result<Checked, CommandError> {
    if let Err(faults) = spec.check() {
        let relevant: Vec<String> = faults
            .iter()
            .filter(|fault| !matches!(fault, SpecFault::WrongName { .. }))
            .map(ToString::to_string)
            .collect();
        if !relevant.is_empty() {
            return Err(invalid(relevant.join("; ")));
        }
    }
    let gateway = reqwest::Url::parse(&spec.gateway_url).map_err(|error| {
        invalid(format!(
            "the gateway {:?} is no address: {error}",
            spec.gateway_url
        ))
    })?;
    if !GATEWAY_SCHEMES.contains(&gateway.scheme()) || gateway.host_str().is_none() {
        return Err(invalid(format!(
            "the gateway {} is not a wss or https address",
            spec.gateway_url
        )));
    }
    let authority = Certificate::from_pem(&spec.ca_cert)
        .map_err(|error| invalid(format!("the certificate authority cannot be read: {error}")))?;
    let text = spec.enroll_token.expose().trim();
    let token: EnrollmentToken = text.parse().map_err(|_| {
        invalid("the enrollment token is not one: it is `oenr_`, a secret, a dot and a pin")
    })?;
    if !token.pins(&authority.spki_sha256()) {
        return Err(invalid(
            "the enrollment token was made by another authority than the certificate given",
        ));
    }
    if tools.current_connector().is_none() {
        return Err(CommandError::new(
            ErrorCode::NotFound,
            "this host has no connector to put into a unit: node.tool.ensure for orch-connector comes first",
        ));
    }
    for tool in &spec.tools {
        if !tools.has(&tool.name, &tool.version) {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                format!(
                    "{} {} is not in the tool cache of this host: node.tool.ensure comes first",
                    tool.name, tool.version
                ),
            ));
        }
    }
    Ok(Checked {
        token: Hash::of(text.as_bytes()),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::RuntimeKind;
    use orch_pki::ca::CertificateAuthority;
    use orch_pki::enrollment::issue_token;
    use orch_pki::sealing::{RecipientKey, info_for_command, seal};
    use orch_types::{ManualClock, new_id};
    use serde_json::json;

    struct World {
        authority: CertificateAuthority,
        clock: ManualClock,
    }

    impl World {
        fn new() -> Self {
            let clock = ManualClock::new(
                "2026-10-11T12:00:00Z"
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .unwrap(),
            );
            Self {
                authority: CertificateAuthority::create("Test", 3650, &clock).unwrap(),
                clock,
            }
        }

        fn token(&self) -> String {
            issue_token(
                &self.authority,
                std::time::Duration::from_secs(900),
                &self.clock,
            )
            .token
            .to_string()
        }

        fn spec(&self, token: &str) -> RuntimeSpec {
            let id = new_id();
            serde_json::from_value(json!({
                "runtime_id": id, "kind": "environment",
                "name": orch_driver_contract::runtime::unit_name(RuntimeKind::Environment, &id),
                "driver": "memory", "spec": {}, "resources": {}, "roles": ["environment"],
                "gateway_url": "wss://gateway.orch.internal:9443",
                "enroll_token": token, "ca_cert": self.authority.certificate().pem(),
                "tools": [], "volumes": {
                    "workspace": { "name": "w", "storage": null },
                    "home": { "name": "h", "storage": null },
                    "state": { "name": "s", "storage": null }
                },
                "mounts": [], "env": {},
                "labels": orch_driver_contract::runtime::unit_labels("sys", RuntimeKind::Environment, &id)
            }))
            .unwrap()
        }
    }

    /// A cache with a connector in it, as `node.tool.ensure` leaves one.
    fn cache_with_connector(dir: &std::path::Path) -> ToolCache {
        let settings = crate::settings::NodeSettings::in_dir(dir);
        let cache = ToolCache::open(&settings).unwrap();
        let version = dir.join("tools/orch-connector/0.1.0");
        std::fs::create_dir_all(&version).unwrap();
        std::os::unix::fs::symlink("0.1.0", dir.join("tools/orch-connector/current")).unwrap();
        cache
    }

    #[test]
    fn a_request_whose_token_is_a_token_of_its_authority_is_accepted_and_only_its_hash_is_kept() {
        let (world, dir) = (World::new(), tempfile::tempdir().unwrap());
        let cache = cache_with_connector(dir.path());
        let token = world.token();
        let checked = check(&world.spec(&token), &cache).unwrap();
        assert_eq!(checked.token, Hash::of(token.as_bytes()));
    }

    #[test]
    fn a_token_of_another_authority_a_bad_gateway_and_a_missing_tool_are_each_refused_before_any_unit_is_made()
     {
        let (world, dir) = (World::new(), tempfile::tempdir().unwrap());
        let cache = cache_with_connector(dir.path());
        let strange = World::new();
        let error = check(&world.spec(&strange.token()), &cache).unwrap_err();
        assert_eq!(error.code, INVALID);
        assert!(
            error.detail.contains("another authority"),
            "{}",
            error.detail
        );

        let mut plain = world.spec(&world.token());
        plain.gateway_url = "ws://gateway.orch.internal:9443".into();
        assert!(
            check(&plain, &cache)
                .unwrap_err()
                .detail
                .contains("wss or https")
        );

        let mut tooled = world.spec(&world.token());
        tooled.tools = vec![orch_driver_contract::runtime::ToolRef {
            name: "codex".into(),
            version: "0.160.1".into(),
        }];
        let error = check(&tooled, &cache).unwrap_err();
        assert_eq!(error.code, "not_found");
        assert!(error.detail.contains("node.tool.ensure"));

        let mut garbled = world.spec("not a token");
        garbled.ca_cert = world.authority.certificate().pem();
        assert!(
            check(&garbled, &cache)
                .unwrap_err()
                .detail
                .contains("not one")
        );
    }

    #[test]
    fn a_host_with_no_connector_in_its_cache_refuses_a_unit() {
        let (world, dir) = (World::new(), tempfile::tempdir().unwrap());
        let cache = ToolCache::open(&crate::settings::NodeSettings::in_dir(dir.path())).unwrap();
        let error = check(&world.spec(&world.token()), &cache).unwrap_err();
        assert_eq!(error.code, "not_found");
        assert!(error.detail.contains("orch-connector"));
    }

    #[test]
    fn the_token_is_opened_with_the_key_it_was_sealed_to_for_the_command_it_came_with() {
        let world = World::new();
        let (key, certificate) = world
            .authority
            .issue_with_new_key(&orch_pki::ca::Holder::Machine(new_id()), 365, &world.clock)
            .unwrap();
        let keys = Keys::new(key);
        let command = new_id();
        let token = world.token();
        let sealed = seal(
            &RecipientKey::from_certificate(&certificate),
            &info_for_command(command),
            token.as_bytes(),
        )
        .unwrap();
        let mut body = serde_json::to_value(world.spec("placeholder")).unwrap();
        body["enroll_token"] = json!({ "$sealed": sealed });
        let spec = unseal(&keys, command, body.clone()).unwrap();
        assert_eq!(spec.enroll_token.expose(), token);
        assert!(
            unseal(&keys, new_id(), body.clone()).is_err(),
            "it opens for no other command"
        );
        body["enroll_token"] = json!(token);
        assert!(
            unseal(&keys, command, body.clone())
                .unwrap_err()
                .detail
                .contains("sealed")
        );
        body["enroll_token"] = json!({ "$secret": new_id() });
        assert!(
            unseal(&keys, command, body)
                .unwrap_err()
                .detail
                .contains("reference")
        );
    }
}
