//! A bounded, explicitly configured connection to the Kubernetes API.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use k8s_openapi::api::core::v1::{Namespace, Pod};
use k8s_openapi::api::node::v1::RuntimeClass as NativeRuntimeClass;
use k8s_openapi::api::storage::v1::StorageClass;
use kube::api::ListParams;
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Api, Client, Config};
use orch_driver_contract::driver::DriverError;

use crate::config::KubernetesConfig;

const HTTP_UNAUTHORIZED: u16 = 401;
const HTTP_FORBIDDEN: u16 = 403;

/// Bind receipts to this endpoint and namespace without persisting the connection string.
fn receipt_scope(endpoint: &str, namespace_uid: &str) -> String {
    let mut input = b"orch-kubernetes-receipts-v1\0".to_vec();
    input.extend_from_slice(&(endpoint.len() as u64).to_be_bytes());
    input.extend_from_slice(endpoint.as_bytes());
    input.extend_from_slice(&(namespace_uid.len() as u64).to_be_bytes());
    input.extend_from_slice(namespace_uid.as_bytes());
    orch_types::Hash::of(&input).to_string()
}

fn read_error(resource: &str, error: &kube::Error) -> DriverError {
    match error {
        kube::Error::Api(response) if response.code == HTTP_UNAUTHORIZED => {
            DriverError::Unavailable("the Kubernetes credentials were refused".into())
        }
        kube::Error::Api(response) if response.code == HTTP_FORBIDDEN => {
            DriverError::Unavailable(format!("the Kubernetes identity cannot read {resource}"))
        }
        _ => DriverError::Unavailable(format!("the Kubernetes API could not read {resource}")),
    }
}

async fn read<T>(
    timeout: Duration,
    resource: &str,
    operation: impl Future<Output = Result<T, kube::Error>>,
) -> Result<T, DriverError> {
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|error| read_error(resource, &error))
}

/// The cluster the operator selected, with the runtime classes currently observed there.
#[derive(Clone)]
pub struct Cluster {
    /// Namespaced and cluster-scoped operations use the same authenticated client.
    pub client: Client,
    /// Configured namespace and class mappings.
    pub configuration: KubernetesConfig,
    /// Immutable identity of the configured namespace, used to scope owned receipts.
    pub namespace_uid: String,
    /// Private journal scope binds the verified API endpoint and namespace incarnation.
    pub receipt_scope: String,
    /// Named RuntimeClasses observed at connection time; refresh before creating a unit.
    pub classes: BTreeSet<String>,
    request_timeout: Duration,
}

impl Cluster {
    /// Connects only through the selected kubeconfig or the in-cluster identity.
    pub async fn connect(
        configuration: KubernetesConfig,
        request_timeout: Duration,
    ) -> Result<Self, DriverError> {
        configuration.check().map_err(|faults| {
            DriverError::InvalidSpec(
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        let mut settings = match &configuration.kubeconfig {
            Some(path) => {
                let kubeconfig = Kubeconfig::read_from(path).map_err(|_| {
                    DriverError::Unavailable("the configured kubeconfig cannot be read".into())
                })?;
                tokio::time::timeout(
                    request_timeout,
                    Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()),
                )
                .await
                .map_err(|_| DriverError::Timeout)?
                .map_err(|_| {
                    DriverError::Unavailable("the configured kubeconfig cannot be used".into())
                })?
            }
            None => Config::incluster().map_err(|_| {
                DriverError::Unavailable("the in-cluster Kubernetes identity is unavailable".into())
            })?,
        };
        if settings.cluster_url.scheme_str() != Some("https") || settings.accept_invalid_certs {
            return Err(DriverError::InvalidSpec(
                "the Kubernetes API must use HTTPS with a verified certificate".into(),
            ));
        }
        let endpoint = settings.cluster_url.to_string();
        settings.connect_timeout = Some(request_timeout);
        // Exec and log streams must not be cut off by a whole-response timeout while data is flowing.
        settings.read_timeout = None;
        settings.write_timeout = None;
        // Kube's default TLS builder uses rustls' process provider. Feature
        // unification may include more than one provider, so install Orch's
        // ring provider only when the application has not chosen one already.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            return Err(DriverError::Unavailable(
                "the Kubernetes TLS provider is unavailable".into(),
            ));
        }
        let client = Client::try_from(settings).map_err(|_| {
            DriverError::Unavailable("the Kubernetes API client cannot be created".into())
        })?;
        let namespaces: Api<Namespace> = Api::all(client.clone());
        let namespace = read(
            request_timeout,
            "the configured namespace",
            namespaces.get_opt(&configuration.namespace),
        )
        .await?
        .ok_or_else(|| {
            DriverError::Unavailable("the configured namespace does not exist".into())
        })?;
        let namespace_uid = namespace
            .metadata
            .uid
            .filter(|uid| !uid.is_empty())
            .ok_or_else(|| {
                DriverError::Unavailable("the Kubernetes API did not identify the namespace".into())
            })?;
        let pods: Api<Pod> = Api::namespaced(client.clone(), &configuration.namespace);
        read(
            request_timeout,
            "the configured namespace",
            pods.list(&ListParams::default().limit(1)),
        )
        .await?;
        let storages: Api<StorageClass> = Api::all(client.clone());
        if read(
            request_timeout,
            "the configured storage class",
            storages.get_opt(&configuration.storage_class),
        )
        .await?
        .is_none()
        {
            return Err(DriverError::Unavailable(
                "the configured storage class does not exist".into(),
            ));
        }
        let mut cluster = Self {
            client,
            configuration,
            receipt_scope: receipt_scope(&endpoint, &namespace_uid),
            namespace_uid,
            classes: BTreeSet::new(),
            request_timeout,
        };
        cluster.refresh_classes().await?;
        Ok(cluster)
    }

    /// Rechecks the operator-admitted named classes against the cluster API.
    pub async fn refresh_classes(&mut self) -> Result<(), DriverError> {
        let api: Api<NativeRuntimeClass> = Api::all(self.client.clone());
        let mut found = BTreeSet::new();
        for class in &self.configuration.runtime_classes {
            if let Some(name) = &class.class_name
                && read(self.request_timeout, "runtime classes", api.get_opt(name))
                    .await?
                    .is_some()
            {
                found.insert(name.clone());
            }
        }
        self.classes = found;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_refusal_reports_permission_without_exposing_the_server_message() {
        let response = kube::core::Status::failure("sensitive detail", "Forbidden")
            .with_code(403)
            .boxed();
        let error = read_error("the configured storage class", &kube::Error::Api(response));
        assert!(
            error
                .to_string()
                .contains("cannot read the configured storage class")
        );
        assert!(!error.to_string().contains("sensitive detail"));
    }

    #[test]
    fn receipt_scopes_change_with_the_endpoint_or_namespace_incarnation() {
        let first = receipt_scope("https://cluster.example:6443", "namespace-one");
        assert_eq!(
            first,
            receipt_scope("https://cluster.example:6443", "namespace-one")
        );
        assert_ne!(
            first,
            receipt_scope("https://other.example:6443", "namespace-one")
        );
        assert_ne!(
            first,
            receipt_scope("https://cluster.example:6443", "namespace-two")
        );
        assert!(!first.contains("cluster.example"));
    }
}
