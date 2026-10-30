//! Defaults that something other than the registry supplies.
//!
//! A key whose default is not a fixed value takes it from somewhere, and that
//! value sits where the registry's default would, under every row. The
//! environment a thread runs in supplies its approval mode and its sandbox
//! (from its template, or from what a connected machine is), and an
//! environment's own exec offers (from its template, or on a person's machine
//! from the local policy that machine reported); a thread's model supplies its default
//! effort; what the system was seeded with supplies the template to make
//! things from, the ready-made provider endpoints and the policy of each
//! capability; the installation supplies whether an address has to be
//! confirmed, by whether it has a mail server.
//!
//! Each is found only for a key that is asked for, at a place where it means
//! something, and with the statements it needs and no more. A key nobody
//! supplies for is unset, which the registry says what it then means.

use std::collections::HashMap;

use orch_db::Conn;
use orch_db::queries::settings as queries;
use orch_db::seeds;
use orch_settings_contract::registry::{DefaultOf, SettingDef, keys};
use orch_types::{Id, Json};
use serde_json::json;

use crate::Error;

/// Approval and sandbox proposed by the environment's declared confinement.
/// Unreported facts use the same conservative defaults as a bare directory.
/// The approval mode is `on_request` everywhere: where another boundary
/// confines the agent, Bypass is offered, never chosen for it.
#[must_use]
pub fn agent_defaults(confinement: &str) -> (&'static str, &'static str) {
    if confinement == "isolated" {
        ("on_request", "open")
    } else {
        ("on_request", "workspace")
    }
}

/// Every key that this module supplies a default for. The registry has no
/// other key whose default is supplied; a test holds that to the registry.
pub const SUPPLIED: &[&str] = &[
    keys::APPROVAL_MODE,
    keys::SANDBOX_MODE,
    keys::EXEC_OFFERS,
    keys::EXEC_ORCH_TOOLS,
    keys::AGENT_EFFORT,
    keys::ENV_TEMPLATE_ID,
    keys::RUNTIME_TEMPLATE_ID,
    keys::PROVIDERS_PRESETS,
    keys::CAPABILITY_POLICY,
    keys::REGISTRATION_EMAIL_VERIFICATION,
];

/// What is known of the place, for the suppliers to read from.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The thread, where the place is one.
    pub thread: Option<Id>,
    /// The environment, where the place is one.
    pub environment: Option<&'a queries::EnvironmentRow>,
    /// The model the thread resolves to, where it resolves to one and nothing
    /// sets its effort: the effort then defaults to what the model lists.
    pub model_for_effort: Option<&'a str>,
}

/// The defaults found for the keys asked for.
#[derive(Debug, Clone, Default)]
pub struct Supplies {
    values: HashMap<&'static str, Json>,
}

impl Supplies {
    /// The default supplied for `key`, where there is one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.values.get(key)
    }
}

/// The text a managed environment's template names for `key`, or what a
/// connected machine is: the environment supplies the default of the keys
/// about how an agent is confined and how an exec node is launched.
pub fn from_environment(key: &str, confinement: &str, spec: Option<&Json>) -> Option<Json> {
    let (approval, sandbox) = agent_defaults(confinement);
    match key {
        keys::APPROVAL_MODE => Some(
            spec.and_then(|s| s.pointer("/agent_defaults/approval.mode"))
                .cloned()
                .unwrap_or_else(|| json!(approval)),
        ),
        keys::SANDBOX_MODE => Some(
            spec.and_then(|s| s.pointer("/sandbox/default"))
                .cloned()
                .unwrap_or_else(|| json!(sandbox)),
        ),
        keys::EXEC_OFFERS => spec?.get("exec")?.get("offers").cloned(),
        keys::EXEC_ORCH_TOOLS => Some(
            spec.and_then(|s| s.pointer("/exec/orch_tools"))
                .cloned()
                .unwrap_or(json!(true)),
        ),
        _ => None,
    }
}

/// Finds the defaults of the keys in `defs` that are supplied, reading only
/// what they need, once each.
pub async fn load(
    conn: &mut Conn,
    context: &Context<'_>,
    defs: &[&'static SettingDef],
) -> Result<Supplies, Error> {
    let mut found = Supplies::default();
    let mut thread_environment: Option<Option<queries::ThreadEnvironment>> = None;
    for def in defs {
        if !matches!(def.default, DefaultOf::Supplied(_)) {
            continue;
        }
        let value = match def.key {
            keys::ENV_TEMPLATE_ID => Some(json!(seeds::DEFAULT_TEMPLATE_ID)),
            keys::RUNTIME_TEMPLATE_ID => Some(json!(seeds::PROVIDER_RUNTIME_TEMPLATE_ID)),
            keys::PROVIDERS_PRESETS => Some(seeds::provider_presets()),
            keys::CAPABILITY_POLICY => {
                let policy: serde_json::Map<String, Json> = queries::capability_defaults(conn)
                    .await?
                    .into_iter()
                    .map(|each| {
                        (
                            each.key,
                            json!({
                                "decision": each.default_decision,
                                "approver_scope": each.default_approver_scope,
                            }),
                        )
                    })
                    .collect();
                Some(Json::Object(policy))
            }
            keys::REGISTRATION_EMAIL_VERIFICATION => {
                let mail = queries::system_has_mail_server(conn).await?;
                Some(json!(if mail { "required" } else { "off" }))
            }
            keys::APPROVAL_MODE | keys::SANDBOX_MODE => match context.thread {
                Some(thread) => {
                    if thread_environment.is_none() {
                        thread_environment = Some(queries::thread_environment(conn, thread).await?);
                    }
                    if let Some(environment) = thread_environment.as_ref().and_then(Option::as_ref)
                    {
                        let machine = orch_db::queries::environment_facts::of(
                            conn,
                            environment.environment_id,
                        )
                        .await?;
                        from_environment(
                            def.key,
                            machine.as_ref().map_or("none", |m| m.confinement()),
                            environment.template_spec.as_ref(),
                        )
                    } else {
                        None
                    }
                }
                None => None,
            },
            keys::EXEC_OFFERS | keys::EXEC_ORCH_TOOLS => match context.environment {
                // On a person's machine, what its exec node may offer is what the machine's own
                // policy lets it offer, as the machine last reported it.
                Some(each) => {
                    let supplied = from_environment(def.key, "none", each.template_spec.as_ref());
                    if def.key == keys::EXEC_OFFERS && supplied.is_none() {
                        queries::machine_exec_offers(conn, each.id)
                            .await?
                            .filter(Json::is_array)
                    } else {
                        supplied
                    }
                }
                None => None,
            },
            keys::AGENT_EFFORT => match (context.thread, context.model_for_effort) {
                (Some(thread), Some(model)) => {
                    queries::thread_model_parameters(conn, thread, model)
                        .await?
                        .and_then(|listed| listed.get("default_effort").cloned())
                        .filter(Json::is_string)
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(value) = value {
            found.values.insert(def.key, value);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_settings_contract::registry::all;

    #[test]
    fn every_key_whose_default_is_supplied_has_somebody_who_supplies_it() {
        for def in all() {
            if matches!(def.default, DefaultOf::Supplied(_)) {
                assert!(
                    SUPPLIED.contains(&def.key),
                    "{} takes its default from somewhere and nothing here supplies it",
                    def.key
                );
            }
        }
        for key in SUPPLIED {
            let def = orch_settings_contract::registry::get(key).unwrap();
            assert!(
                matches!(def.default, DefaultOf::Supplied(_)),
                "{key} is not supplied"
            );
        }
    }

    #[test]
    fn a_template_names_the_defaults_of_its_environment_and_a_machine_has_its_own() {
        let spec = json!({
            "agent_defaults": { "approval.mode": "bypass" },
            "sandbox": { "default": "open", "max": "open" },
            "exec": { "offers": ["plain"], "orch_tools": false }
        });
        assert_eq!(
            from_environment(keys::APPROVAL_MODE, "isolated", Some(&spec)),
            Some(json!("bypass"))
        );
        assert_eq!(
            from_environment(keys::SANDBOX_MODE, "isolated", Some(&spec)),
            Some(json!("open"))
        );
        assert_eq!(
            from_environment(keys::EXEC_OFFERS, "isolated", Some(&spec)),
            Some(json!(["plain"]))
        );
        assert_eq!(
            from_environment(keys::EXEC_ORCH_TOOLS, "isolated", Some(&spec)),
            Some(json!(false))
        );
        assert_eq!(
            from_environment(keys::APPROVAL_MODE, "none", Some(&json!({}))),
            Some(json!("on_request"))
        );
        assert_eq!(
            from_environment(keys::APPROVAL_MODE, "none", None),
            Some(json!("on_request"))
        );
        assert_eq!(
            from_environment(keys::SANDBOX_MODE, "none", None),
            Some(json!("workspace"))
        );
        assert_eq!(from_environment(keys::EXEC_OFFERS, "none", None), None);
        for key in [keys::APPROVAL_MODE, keys::SANDBOX_MODE] {
            assert_eq!(
                from_environment(key, "none", Some(&json!({}))),
                from_environment(key, "none", None)
            );
        }
    }
}
