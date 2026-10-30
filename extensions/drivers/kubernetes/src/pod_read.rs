//! Read the admitted Pod's spec without losing fields unknown to the typed client.

use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use kube::api::{ApiResource, DynamicObject};
use kube::{Api, Client};
use orch_driver_contract::driver::DriverError;
use orch_types::Json;

/// The dynamic object preserves unknown spec members for the admission comparison.
pub(crate) async fn read_pod(
    client: Client,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> Result<Option<Json>, DriverError> {
    let resource = ApiResource::erase::<Pod>(&());
    let pods: Api<DynamicObject> = Api::namespaced_with(client, namespace, &resource);
    let found = tokio::time::timeout(timeout, pods.get_opt(name))
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Unavailable("the managed Pod could not be read".into()))?;
    found
        .map(|pod| {
            serde_json::to_value(pod).map_err(|_| {
                DriverError::Internal("the admitted Pod could not be inspected".into())
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_unrecognized_future_pod_spec_field_is_preserved_for_refusal() {
        let original = json!({
            "apiVersion":"v1", "kind":"Pod", "metadata":{"name":"example"},
            "spec":{"containers":[], "futureField":{"unrecognized":true}}
        });
        let pod: DynamicObject = serde_json::from_value(original).unwrap();
        let read = serde_json::to_value(pod).unwrap();
        assert_eq!(read["spec"]["futureField"]["unrecognized"], true);
    }
}
