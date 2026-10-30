//! The Kubernetes driver as the node uses it: the machine policy's connection object, and the few things
//! the node asks of a driver beyond the contract.
//!
//! A machine offers Kubernetes when its policy allows the driver `kubernetes` and names a connection object for
//! it. The object is checked against the schema the driver reports (so that a fault shows beside the field
//! before anything is activated), the cluster is contacted when the driver loads, and only a driver that
//! reached its cluster and passed its probes joins the host: a declared machine whose cluster is down still
//! reports the schema, and offers nothing to schedule.

use std::sync::Arc;

use async_trait::async_trait;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::units::UnitRef;
use orch_driver_contract::volumes::VolumeRequest;
use orch_driver_kubernetes::KubernetesDriver;
use orch_driver_kubernetes::config::KubernetesConfig;
use orch_node_core::drivers::{DriverExtras, Factory, HostDriver, Registrations};
use orch_types::Json;

/// The name the driver goes by in the machine policy and in a template.
pub const NAME: &str = "kubernetes";

/// The typed connection object, checked as a whole. A fault never echoes what was written: the object may sit
/// beside a credential path, and the schema's own faults already say which field is wrong.
fn connection_of(value: &Json) -> Result<KubernetesConfig, DriverError> {
    let config: KubernetesConfig = serde_json::from_value(value.clone()).map_err(|_| {
        DriverError::InvalidSpec(
            "the Kubernetes connection settings do not match the driver's schema".into(),
        )
    })?;
    config.check().map_err(|faults| {
        DriverError::InvalidSpec(
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    Ok(config)
}

/// What the node asks of the driver beyond the contract.
struct KubernetesExtras {
    driver: Arc<KubernetesDriver>,
}

#[async_trait]
impl DriverExtras for KubernetesExtras {
    async fn stop_volume_work(&self, command: orch_types::Id) -> Result<(), DriverError> {
        self.driver.stop_volume_work(command).await
    }

    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        self.driver.check_volume_carry().await
    }

    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        self.driver.volume_exists(volume).await
    }

    async fn enrolled(&self, unit: &UnitRef) -> Option<bool> {
        self.driver.connector_enrolled(unit).await
    }
}

fn factory(schema: Json) -> Factory {
    Arc::new(move |context| {
        let schema = schema.clone();
        Box::pin(async move {
            let Some(connection) = &context.configuration else {
                return Err(DriverError::InvalidSpec(
                    "the machine's driver settings name no Kubernetes cluster yet".into(),
                ));
            };
            let config = connection_of(connection)?;
            let driver = KubernetesDriver::connect(
                config,
                &context.settings.data_dir,
                &context.settings.tools_dir(),
                &context.settings.connector_dir(),
            )
            .await?;
            let contract: Arc<dyn EnvironmentDriver> = driver.clone();
            let reported = schema.clone();
            Ok(HostDriver::new(contract)
                .with_extras(Arc::new(KubernetesExtras { driver }))
                .with_configuration(
                    reported,
                    Arc::new(move |value| {
                        orch_plugin::schema::validate(&schema, value)
                            .map_err(|error| DriverError::InvalidSpec(error.to_string()))?;
                        connection_of(value).map(|_| ())
                    }),
                ))
        })
    })
}

/// Adds the Kubernetes driver, with its setup schema reported even while the cluster cannot be reached.
pub fn register(registrations: Registrations) -> anyhow::Result<Registrations> {
    let schema = orch_driver_kubernetes::configuration_schema()
        .map_err(|error| anyhow::anyhow!("the Kubernetes setup schema: {error}"))?;
    Ok(registrations.register_with_schema(NAME, factory(schema.clone()), schema))
}
