//! Role and policy handover choices bound to an enrollment token's existing secret hash.

use crate::{
    enrollment::{EnrollmentToken, NotAToken},
    session::Role,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A connector never advertises a role implemented by the separate node program.
pub const CONNECTOR_NODE_REFUSAL: &str =
    "The node role is hosted by orch-node: add this computer as a host as well.";

/// Parses the local connector's role list without advertising a factory this program does not host.
pub fn connector_roles(text: &str) -> Result<Vec<Role>, &'static str> {
    let mut roles = Vec::new();
    for word in text.split(',').map(str::trim) {
        roles.push(match word.to_ascii_lowercase().as_str() {
            "environment" => Role::Environment,
            "agent" => Role::Agent,
            "provider" => Role::Provider,
            "node" => return Err(CONNECTOR_NODE_REFUSAL),
            _ => return Err("Choose environment, agent or provider as the connector's roles."),
        });
    }
    EnrollmentChoices::new(roles.clone(), Vec::new(), false)?;
    Ok(roles)
}
// The versioned suffix is part of the hashed secret; changing any choice invalidates the token.
const CHOICES_MARKER: &str = ".roles1.";
// Four roles and their four section selectors fit comfortably; bound malformed token metadata too.
const CHOICES_MAX_BYTES: usize = 2048;

/// Choices bound to this enrollment; later role edits obey the separate machine.roles handover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentChoices {
    /// The roles of this one program; Node cannot combine with connector roles.
    pub roles: Vec<Role>,
    /// Whole role sections (environment.*, agent.*, provider.*, node.*), or * for all policy.
    pub delegate: Vec<String>,
    /// The operator's local declaration that this machine holds nothing else.
    pub dedicated: bool,
}

impl EnrollmentChoices {
    /// Validates roles and normalizes section shorthand to the single policy's selectors.
    pub fn new(
        roles: Vec<Role>,
        delegate: Vec<String>,
        dedicated: bool,
    ) -> Result<Self, &'static str> {
        if roles.is_empty()
            || roles
                .iter()
                .enumerate()
                .any(|(index, role)| roles[..index].contains(role))
        {
            return Err("Choose at least one role, without repeating a role.");
        }
        if roles.contains(&Role::Node) && roles.len() != 1 {
            return Err(CONNECTOR_NODE_REFUSAL);
        }
        let mut sections = Vec::new();
        for section in delegate {
            let section = match section.trim() {
                "*" | "all" => "*",
                "environment" | "environment.*" => "environment.*",
                "agent" | "agent.*" => "agent.*",
                "provider" | "provider.*" => "provider.*",
                "node" | "node.*" => "node.*",
                _ => return Err("Hand over a selected role section, or all policy with *."),
            };
            let present = match section {
                "environment.*" => {
                    roles.contains(&Role::Environment) || roles.contains(&Role::Node)
                }
                "agent.*" => roles.contains(&Role::Agent),
                "provider.*" => roles.contains(&Role::Provider),
                "node.*" => roles.contains(&Role::Node),
                _ => true,
            };
            if !present {
                return Err("Hand over only the sections of the roles this program offers.");
            }
            if !sections.iter().any(|value| value == section) {
                sections.push(section.to_owned());
            }
        }
        if sections.iter().any(|section| section == "*") {
            sections = vec!["*".into()];
        }
        sections.sort();
        Ok(Self {
            roles,
            delegate: sections,
            dedicated,
        })
    }

    /// Compares consent independently of the order roles were checked or flags were written.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.roles.len() == other.roles.len()
            && self.roles.iter().all(|role| other.roles.contains(role))
            && self.delegate == other.delegate
            && self.dedicated == other.dedicated
    }
}

impl EnrollmentToken {
    /// Binds choices inside the opaque secret before its hash is stored; the authority pin stays intact.
    pub fn with_choices(&self, choices: &EnrollmentChoices) -> Result<Self, NotAToken> {
        if self.secret().contains(CHOICES_MARKER) {
            return Err(NotAToken);
        }
        let choices = EnrollmentChoices::new(
            choices.roles.clone(),
            choices.delegate.clone(),
            choices.dedicated,
        )
        .map_err(|_| NotAToken)?;
        let bytes = serde_json::to_vec(&choices).map_err(|_| NotAToken)?;
        if bytes.len() > CHOICES_MAX_BYTES {
            return Err(NotAToken);
        }
        Ok(Self::new(
            &format!(
                "{}{CHOICES_MARKER}{}",
                self.secret(),
                URL_SAFE_NO_PAD.encode(bytes)
            ),
            *self.pin(),
        ))
    }

    /// Reads choices for local confirmation; only the server's token-hash check authenticates them.
    pub fn choices(&self) -> Result<Option<EnrollmentChoices>, NotAToken> {
        let Some((_, encoded)) = self.secret().rsplit_once(CHOICES_MARKER) else {
            return Ok(None);
        };
        if encoded.len() > CHOICES_MAX_BYTES {
            return Err(NotAToken);
        }
        let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| NotAToken)?;
        let raw: EnrollmentChoices = serde_json::from_slice(&bytes).map_err(|_| NotAToken)?;
        EnrollmentChoices::new(raw.roles, raw.delegate, raw.dedicated)
            .map(Some)
            .map_err(|_| NotAToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_token_has_no_new_choices_and_selected_roles_and_sections_round_trip_with_the_same_pin()
     {
        let token = EnrollmentToken::new("legacy-secret", [7; 32]);
        assert_eq!(token.choices().unwrap(), None);
        let choices = EnrollmentChoices::new(
            vec![Role::Environment, Role::Agent],
            vec!["environment".into()],
            true,
        )
        .unwrap();
        let bound = token.with_choices(&choices).unwrap();
        let read: EnrollmentToken = bound.to_string().parse().unwrap();
        assert_eq!(read.choices().unwrap(), Some(choices));
        assert_eq!(read.pin(), token.pin());
        assert_ne!(read.secret(), token.secret());
        assert!(EnrollmentChoices::new(vec![Role::Agent], vec!["provider".into()], false).is_err());
        assert!(EnrollmentChoices::new(vec![Role::Node, Role::Agent], Vec::new(), false).is_err());
    }
}
