//! Template fields accepted by a Kubernetes environment.

use orch_types::Json;
use serde_json::json;

/// The schema stays independent of a particular cluster's discovered RuntimeClasses.
pub(crate) fn environment(restricted_verified: bool) -> Json {
    let limits = json!({"type":"object","additionalProperties":false,"properties":{
        "cpu_millis":{"type":"integer","minimum":1},
        "memory_mb":{"type":"integer","minimum":1}
    }});
    let needs = json!({"type":"object","additionalProperties":false,"properties":{
        "cpu_millis":{"type":"integer","minimum":0},
        "memory_mb":{"type":"integer","minimum":0},
        "disk_gb":{"type":"integer","minimum":0}
    }});
    let example = json!({"image":"node:22-bookworm", "workspace_mount":"/workspace", "home_mount":"/home/agent",
        "network":{"mode":"bridge","egress":"open","dns":[]}, "security":{"no_new_privileges":true},
        "resources":{"cpu_millis":4000,"memory_mb":4096}, "requests":{"cpu_millis":250,"memory_mb":256},
        "env":{}, "host_labels":[], "after_up":[], "exec":{"offers":["plain"],"orch_tools":true}});
    let network_modes = if restricted_verified {
        vec!["bridge", "none"]
    } else {
        vec!["bridge"]
    };
    let egress_modes = if restricted_verified {
        vec!["open", "restricted"]
    } else {
        vec!["open"]
    };
    json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "title":"A Kubernetes environment", "type":"object", "additionalProperties":false,
        "required":["image"],
        "properties":{
            "image":{"type":"string","minLength":1,"title":"Image reference"},
            "image_digest":{"type":["string","null"],"pattern":"^sha256:[0-9a-f]{64}$"},
            "runtime":{"type":"string","minLength":1,"x-runtime":true},
            "workspace_mount":{"type":"string","pattern":"^/","default":"/workspace"},
            "home_mount":{"type":"string","pattern":"^/","default":"/home/agent"},
            "resources":limits.clone(), "resources_max":limits, "needs":needs,
            "requests":{"type":"object","additionalProperties":false,"properties":{
                "cpu_millis":{"type":"integer","minimum":0,"default":0,"x-unit":"mCPU"},
                "memory_mb":{"type":"integer","minimum":0,"default":0,"x-unit":"MiB"}}},
            "network":{"type":"object","additionalProperties":false,"properties":{
                "mode":{"enum":network_modes},"egress":{"enum":egress_modes},
                "dns":{"type":"array","maxItems":0}}},
            "security":{"type":"object","additionalProperties":false,"properties":{
                "read_only_rootfs":{"type":"boolean"},"no_new_privileges":{"const":true},
                "cap_drop":{"type":"array","items":{"type":"string"}}}},
            "env":{"type":"object","additionalProperties":{"type":"string"}},
            "host_labels":{"type":"array","items":{"type":"string"}},
            "after_up":orch_settings_contract::unit::after_up_template_schema(),
            "base_capabilities":{"type":"array","items":{"type":"string"}},
            "allowed_capabilities":{"type":"array","items":{"type":"string"}},
            "sandbox":{"type":"object","properties":{"default":{"enum":["read_only","workspace","open"]},
                "max":{"enum":["read_only","workspace","open"]}}},
            "agent_defaults":{"type":"object"},
            "exec":{"type":"object","properties":{"offers":{"type":"array","items":{"type":"string"}},"orch_tools":{"type":"boolean"}}},
            "colocated_harnesses":{"type":"array","items":{"type":"string"}},
            "mcp_server_ids":{"type":"array","items":{"type":"string"}}
        },
        "x-orch-examples":[{"id":"node","name":"Node","title":"Node web project",
            "description":"Use a Node image and request CPU and memory from the Kubernetes scheduler.","spec":example}]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_uses_only_supported_source_and_native_request_fields() {
        let schema = environment(false);
        let validator = jsonschema::validator_for(&schema).unwrap();
        let example = &schema["x-orch-examples"][0]["spec"];
        assert!(validator.is_valid(example));
        assert_eq!(schema["properties"]["runtime"]["x-runtime"], true);
        let mut invalid = example.clone();
        invalid["requests"]["cpu_millis"] = json!(-1);
        assert!(!validator.is_valid(&invalid));
        let mut invalid = example.clone();
        invalid["build"] = json!({"dockerfile":"FROM example"});
        assert!(!validator.is_valid(&invalid));
        let mut restricted = example.clone();
        restricted["network"]["egress"] = json!("restricted");
        assert!(!validator.is_valid(&restricted));
        assert!(
            jsonschema::validator_for(&environment(true))
                .unwrap()
                .is_valid(&restricted)
        );
        let mut escalation = example.clone();
        escalation["security"]["no_new_privileges"] = json!(false);
        assert!(!validator.is_valid(&escalation));
        let mut native = example.clone();
        native["security"]["cap_add"] = json!(["BPF"]);
        assert!(!validator.is_valid(&native));
    }
}
