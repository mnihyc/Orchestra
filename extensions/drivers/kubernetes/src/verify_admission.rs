//! Compare the security-relevant parts of an admitted Pod with the planned Pod.
//! Unknown mutations are refused until their effect is understood.

use orch_driver_contract::driver::DriverError;
use serde_json::Value;

fn refused() -> DriverError {
    DriverError::Unavailable("the cluster changed the managed Pod's isolation settings".into())
}

fn container_field(path: &str, field: &str) -> bool {
    let remainder = path
        .strip_prefix("/spec/containers/")
        .or_else(|| path.strip_prefix("/spec/initContainers/"));
    remainder
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(index, name)| {
            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()) && name == field
        })
}

/// These are API defaults which cannot add a mount, process, credential or host namespace.
/// Node assignment is checked separately against the observed worker's labels and availability.
fn harmless_default(path: &str, value: &Value) -> bool {
    match path {
        "/spec/dnsPolicy" => value == "ClusterFirst",
        "/spec/serviceAccount" | "/spec/serviceAccountName" => value == "default",
        "/spec/schedulerName" => value == "default-scheduler",
        "/spec/terminationGracePeriodSeconds" => value == 30,
        "/spec/enableServiceLinks" => value == true,
        "/spec/priority" => value == 0,
        "/spec/preemptionPolicy" => value == "PreemptLowerPriority",
        "/spec/securityContext" => value.as_object().is_some_and(|object| object.is_empty()),
        "/spec/tolerations" => {
            // The default admission controller tolerates these two node conditions for five minutes.
            let defaults = serde_json::json!([
                {"effect":"NoExecute","key":"node.kubernetes.io/not-ready","operator":"Exists","tolerationSeconds":300},
                {"effect":"NoExecute","key":"node.kubernetes.io/unreachable","operator":"Exists","tolerationSeconds":300}
            ]);
            value == &defaults
        }
        _ if container_field(path, "terminationMessagePath") => value == "/dev/termination-log",
        _ if container_field(path, "terminationMessagePolicy") => value == "File",
        _ if container_field(path, "resources") => {
            value.as_object().is_some_and(|object| object.is_empty())
        }
        _ => false,
    }
}

fn compare(expected: &Value, actual: &Value, path: &str) -> bool {
    match (expected, actual) {
        (Value::Object(wanted), Value::Object(found)) => {
            wanted.iter().all(|(key, value)| {
                found
                    .get(key)
                    .is_some_and(|other| compare(value, other, &format!("{path}/{key}")))
            }) && found.iter().all(|(key, value)| {
                wanted.contains_key(key) || harmless_default(&format!("{path}/{key}"), value)
            })
        }
        (Value::Array(wanted), Value::Array(found)) => {
            wanted.len() == found.len()
                && wanted
                    .iter()
                    .zip(found)
                    .enumerate()
                    .all(|(index, (wanted, found))| {
                        compare(wanted, found, &format!("{path}/{index}"))
                    })
        }
        _ => expected == actual,
    }
}

/// Rejects an admitted request that gained code, privilege, mounts or a different planned setting.
/// The caller must independently verify metadata, the actual worker and all current network policies.
pub(crate) fn same_isolation(
    expected: &Value,
    actual: &Value,
    require_worker: bool,
) -> Result<(), DriverError> {
    // Network and security admission plugins may act on annotations, so compare them too.
    if expected.pointer("/metadata/annotations") != actual.pointer("/metadata/annotations") {
        return Err(refused());
    }
    let mut actual_spec = actual.get("spec").cloned().ok_or_else(refused)?;
    let spec = actual_spec.as_object_mut().ok_or_else(refused)?;
    // The scheduler assigns the worker after admission. Its labels and availability must be checked separately.
    match spec.remove("nodeName") {
        Some(node) if node.as_str().is_none_or(str::is_empty) => return Err(refused()),
        None if require_worker => {
            return Err(DriverError::Unavailable("the Pod has no worker yet".into()));
        }
        _ => {}
    }
    let expected_spec = expected.get("spec").ok_or_else(refused)?;
    if compare(expected_spec, &actual_spec, "/spec") {
        Ok(())
    } else {
        Err(refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(spec: Value) -> Value {
        json!({"apiVersion":"v1", "kind":"Pod", "metadata":{"name":"fixture"}, "spec":spec})
    }

    fn planned() -> Value {
        pod(
            json!({"automountServiceAccountToken":false,"restartPolicy":"Never", "shareProcessNamespace":true,
            "containers":[{"name":"workload", "image":"example:1", "command":["/supervisor"],
                "securityContext":{"allowPrivilegeEscalation":false, "capabilities":{"drop":["ALL"]}}}]}),
        )
    }

    fn admitted() -> Value {
        let mut pod = planned();
        pod["spec"]["nodeName"] = json!("worker-1");
        pod
    }

    #[test]
    fn only_known_harmless_defaults_can_be_added() {
        let expected = planned();
        let mut actual = admitted();
        actual["spec"]["dnsPolicy"] = json!("ClusterFirst");
        actual["spec"]["serviceAccountName"] = json!("default");
        actual["spec"]["tolerations"] = json!([
            {"effect":"NoExecute","key":"node.kubernetes.io/not-ready","operator":"Exists","tolerationSeconds":300},
            {"effect":"NoExecute","key":"node.kubernetes.io/unreachable","operator":"Exists","tolerationSeconds":300}
        ]);
        actual["spec"]["containers"][0]["terminationMessagePath"] = json!("/dev/termination-log");
        assert!(same_isolation(&expected, &actual, true).is_ok());
        assert!(same_isolation(&expected, &expected, true).is_err());
        assert!(same_isolation(&expected, &expected, false).is_ok());
    }

    #[test]
    fn an_injected_command_mount_token_or_host_namespace_is_refused() {
        let expected = planned();
        for patch in [
            json!({"hostNetwork":true}),
            json!({"automountServiceAccountToken":true}),
            json!({"initContainers":[{"name":"injected","image":"example:2"}]}),
            json!({"volumes":[{"name":"injected","hostPath":{"path":"/"}}]}),
            json!({"futureField":{"unrecognized":true}}),
            json!({"tolerations":[{"operator":"Exists"}]}),
        ] {
            let mut altered = admitted();
            altered["spec"]
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(same_isolation(&expected, &altered, true).is_err());
        }
        let mut altered = admitted();
        altered["spec"]["containers"][0]["lifecycle"] =
            json!({"postStart":{"exec":{"command":["/unexpected"]}}});
        assert!(same_isolation(&expected, &altered, true).is_err());
        let mut altered = admitted();
        altered["spec"]["containers"][0]["envFrom"] = json!([{"secretRef":{"name":"secret"}}]);
        assert!(same_isolation(&expected, &altered, true).is_err());
        let mut altered = admitted();
        altered["metadata"]["annotations"] = json!({"k8s.v1.cni.cncf.io/networks":"extra"});
        assert!(same_isolation(&expected, &altered, true).is_err());
        let mut expected_with_env = planned();
        expected_with_env["spec"]["containers"][0]["env"] =
            json!([{"name":"EXAMPLE", "value":"x"}]);
        let mut altered = expected_with_env.clone();
        altered["spec"]["nodeName"] = json!("worker-1");
        altered["spec"]["containers"][0]["env"][0]["terminationMessagePath"] =
            json!("/dev/termination-log");
        assert!(same_isolation(&expected_with_env, &altered, true).is_err());
    }
}
