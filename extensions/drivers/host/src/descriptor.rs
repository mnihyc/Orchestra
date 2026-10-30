//! Host template and provider schemas and confinement facts.
//!
//! A host process keeps its directories and uses the harness's sandbox; only a proved native scope enforces limits.

use orch_driver_contract::descriptor::DriverDescriptor;
use serde_json::json;

/// The descriptor of this host's actually available process and limit implementation.
#[must_use]
pub fn descriptor(scopes: bool) -> DriverDescriptor {
    probed_descriptor(orch_driver_contract::process_limits::ProcessControls {
        cpu: scopes,
        memory: scopes,
        pids: scopes,
    })
}

/// Each enforced resource follows its controller's own probe.
#[must_use]
pub fn probed_descriptor(
    controls: orch_driver_contract::process_limits::ProcessControls,
) -> DriverDescriptor {
    let properties = json!({
        "run_as":{"type":"string","minLength":1,"title":"Run as user","description":"An existing account on every machine using this template. Choose named machine storages in env.storage or runtime.storage; paths stay in machine configuration.","default":null,"examples":["agent"]},
        "after_up":orch_settings_contract::unit::after_up_template_schema(),
        "init":{"type":["object","null"],"deprecated":true,"description":"Superseded: runs as after_up entry init"}
    });
    let ready = json!([{"name":"10-ready","command":["/usr/bin/printf","Host tools are ready."]}]);
    let environment = json!({"type":"object","additionalProperties":false,"properties":properties,"required":["run_as"],"x-orch-examples":[{"id":"host-account","title":"Run tools as a host user","description":"Run printf after start as the existing agent account. Choose an account and installed programs available on your hosts before saving.","spec":{"run_as":"agent","after_up":ready}}]});
    let provider = json!({"type":"object","additionalProperties":false,"properties":{"run_as":properties["run_as"],
        "after_up":orch_settings_contract::unit::after_up_template_schema(),
        "init":{"type":["object","null"],"deprecated":true},
        "plugins":orch_settings_contract::unit::plugins_template_schema()},"required":["run_as"],"x-orch-examples":[{"id":"host-account","title":"Run a provider as a host user","description":"Run printf after start as the existing agent account. Choose an account and installed programs available on your hosts before saving.","spec":{"run_as":"agent","after_up":ready,"plugins":[]}}]});
    let mut schemas = std::collections::BTreeMap::from([
        (
            orch_driver_contract::runtime::RuntimeKind::Environment,
            environment,
        ),
        (
            orch_driver_contract::runtime::RuntimeKind::Provider,
            provider,
        ),
    ]);
    for schema in schemas.values_mut() {
        // Keep other driver/agent settings losslessly; only the removed host-path mechanism is refused.
        schema["additionalProperties"] = json!(true);
        schema["x-orch-storage-selection"] = json!(true);
        schema["not"] = json!({"anyOf":[{"required":["workspace_base"]},{"required":["home_base"]},{"required":["mounts_base"]}]});
        if let Some(examples) = schema
            .get_mut("x-orch-examples")
            .and_then(serde_json::Value::as_array_mut)
        {
            for example in examples {
                example["name"] = json!("Host");
            }
        }
    }
    DriverDescriptor {
        name: "host".into(),
        version: "1".into(),
        title: "Host processes".into(),
        configuration_schema: None,
        host_scope: Some(orch_driver_contract::host::HostScope::Node),
        tool_delivery: Some(orch_driver_contract::host::ToolDelivery::HostDirectory),
        gateway_access: Some(orch_driver_contract::host::GatewayAccess::NodeAddress),
        volume_export_consistency: None,
        host_ports: orch_driver_contract::descriptor::OperationSupport::available(true, ""),
        kept_file_transfer: Default::default(),
        local_accounts: true,
        unavailable: std::collections::BTreeMap::from([
            (
                "private_views".into(),
                "this driver uses directories in place".into(),
            ),
            (
                "suspend".into(),
                "this driver cannot suspend all unit processes".into(),
            ),
        ]),
        spec_schemas: schemas,
        confinement: orch_driver_contract::descriptor::Confinement::None,
        confinement_by: None,
        lifecycle: orch_driver_contract::descriptor::Lifecycle::Unit,
        commands_after_start:
            orch_driver_contract::descriptor::OperationSupport::process_group_commands(cfg!(unix)),
        network_egress: vec![orch_driver_contract::network::EgressMode::Open],
        capabilities: std::collections::BTreeMap::new(),
        recreate: orch_driver_contract::descriptor::Recreate::Restart,
        live_changes: orch_driver_contract::descriptor::LiveChanges {
            gateway_given: orch_driver_contract::capabilities::GATEWAY_GIVEN
                .iter()
                .map(|key| (*key).into())
                .collect(),
            ..Default::default()
        },
        recreate_preserves_files: true,
        volumes: true,
        data_storage: orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
        mounts: orch_driver_contract::mounts::MountPlacement::InPlace,
        private_views: false,
        storages: true,
        suspend: false,
        sources: Vec::new(),
        imports: Vec::new(),
        resource_limits: controls.support(),
        process_scope: orch_driver_contract::descriptor::ProcessScope::Started,
        runtime_classes: None,
        runtimes: Vec::new(),
        default_runtime: None,
        extensions: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_shared_host_template_uses_an_account_and_machine_storage_names_instead_of_host_paths() {
        let descriptor = descriptor(false);
        for schema in descriptor.spec_schemas.values() {
            let validator = jsonschema::validator_for(schema).unwrap();
            assert!(validator.is_valid(&json!({"run_as":"agent"})));
            assert!(validator.is_valid(&json!({"run_as":"agent","extension":{"kept":true}})));
            for key in ["workspace_base", "home_base", "mounts_base"] {
                assert!(schema["properties"].get(key).is_none());
                let mut old = json!({"run_as":"agent"});
                old[key] = json!("/host-specific/path");
                assert!(!validator.is_valid(&old));
            }
            for example in schema["x-orch-examples"].as_array().unwrap() {
                assert!(validator.is_valid(&example["spec"]));
                assert!(orch_settings_contract::unit::template_commands(&example["spec"]).is_ok());
                assert_eq!(example["spec"]["run_as"], "agent");
                assert_eq!(
                    example["spec"]["after_up"][0]["command"][0],
                    "/usr/bin/printf"
                );
            }
        }
    }
}
