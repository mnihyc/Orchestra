//! Validate local connector role and handover consent before enrollment or a role configuration change.

use crate::arguments::Enroll;
use orch_config::ConnectorConfig;
use orch_link_contract::{enrollment_choices::EnrollmentChoices, session::Role};

/// The roles this program actually hosts, accepting comma-separated local command arguments.
pub fn connector_roles(text: &str) -> anyhow::Result<Vec<Role>> {
    orch_link_contract::enrollment_choices::connector_roles(text).map_err(anyhow::Error::msg)
}

/// Resolves explicit command flags against the operator's existing local configuration.
pub fn selected(options: &Enroll, config: &ConnectorConfig) -> anyhow::Result<EnrollmentChoices> {
    anyhow::ensure!(
        !(options.delegate_policy && options.delegate.is_some()),
        "Use --delegate or --delegate-policy, not both."
    );
    let roles = connector_roles(
        options
            .roles
            .as_deref()
            .unwrap_or(&config.connector.roles.join(",")),
    )?;
    let delegate = if options.delegate_policy {
        vec!["*".into()]
    } else {
        options.delegate.as_ref().map_or_else(
            || config.policy.delegated.clone(),
            |sections| {
                sections
                    .split(',')
                    .map(|section| section.trim().to_owned())
                    .collect()
            },
        )
    };
    EnrollmentChoices::new(
        roles,
        delegate,
        options.dedicated || config.connector.dedicated,
    )
    .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::enrollment_choices::CONNECTOR_NODE_REFUSAL;

    #[test]
    fn enrollment_flags_select_one_roles_section_and_dedication_instead_of_delegating_other_roles()
    {
        use clap::Parser as _;
        let args = crate::arguments::Args::try_parse_from([
            "orch-connector",
            "enroll",
            "--url",
            "https://orch.test",
            "--token",
            "oenr_fake.pin",
            "--roles",
            "environment,provider",
            "--delegate",
            "environment",
            "--dedicated",
        ])
        .unwrap();
        let Some(crate::arguments::Command::Enroll(options)) = args.command else {
            panic!("not enrollment");
        };
        let choices = selected(&options, &ConnectorConfig::default()).unwrap();
        assert_eq!(choices.roles, vec![Role::Environment, Role::Provider]);
        assert_eq!(choices.delegate, ["environment.*"]);
        assert!(choices.dedicated);
    }

    #[test]
    fn a_connector_accepts_its_three_roles_and_directs_node_enrollment_to_the_other_program() {
        assert_eq!(
            connector_roles("environment,provider").unwrap(),
            vec![Role::Environment, Role::Provider]
        );
        assert_eq!(
            connector_roles("node").unwrap_err().to_string(),
            CONNECTOR_NODE_REFUSAL
        );
        assert_eq!(
            connector_roles("agent,node").unwrap_err().to_string(),
            CONNECTOR_NODE_REFUSAL
        );
        assert!(connector_roles("").is_err());
        assert!(connector_roles("agent,agent").is_err());
    }
}
