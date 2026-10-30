//! Resource limits changed on the owned container without replacing it or stopping its processes.

use bollard::models::ContainerUpdateBody;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::Resources;
use orch_driver_contract::units::UnitRef;

use crate::client::map_error;
use crate::driver::DockerDriver;

impl DockerDriver {
    pub(crate) async fn update_resources(
        &self,
        unit: &UnitRef,
        resources: &Resources,
    ) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        let previous = container.host_config.as_ref();
        if resources.cpu_millis.is_none()
            && previous
                .and_then(|host| host.nano_cpus)
                .is_some_and(|value| value > 0)
            || resources.memory_mb.is_none()
                && previous
                    .and_then(|host| host.memory)
                    .is_some_and(|value| value > 0)
        {
            return Err(DriverError::Unsupported("clearing a finite Docker CPU or memory bound requires recreation; zero in Docker's update API leaves the old bound unchanged".into()));
        }
        if [
            resources.cpu_millis,
            resources.memory_mb,
            resources.pids,
            resources.disk_gb,
        ]
        .into_iter()
        .flatten()
        .any(|limit| limit <= 0)
        {
            return Err(DriverError::InvalidSpec(
                "a resource limit is positive or absent".into(),
            ));
        }
        if resources
            .memory_swap_mb
            .is_some_and(|swap| swap <= 0 || resources.memory_mb.is_none_or(|memory| swap < memory))
            || resources
                .blkio_weight
                .is_some_and(|weight| !(10..=1000).contains(&weight))
        {
            return Err(DriverError::InvalidSpec("memory plus swap needs a memory limit and may not be smaller; block IO weight is between 10 and 1000".into()));
        }
        if resources.memory_swap_mb.is_some() && !self.facts().swap_limit {
            return Err(DriverError::Unsupported(
                "this daemon cannot enforce the requested swap limit".into(),
            ));
        }
        if !self.facts().limits_enforced() {
            return Err(DriverError::Unsupported(
                "this daemon cannot enforce live resource limits".into(),
            ));
        }
        let mut host = bollard::models::HostConfig::default();
        crate::units::apply_limits(resources, self.facts(), &mut host);
        let memory = host.memory.unwrap_or(0);
        let update = ContainerUpdateBody {
            nano_cpus: Some(host.nano_cpus.unwrap_or(0)),
            memory: Some(memory),
            // As at creation, swap may not spill beyond the configured memory by default.
            // A supported explicit total is applied together with the memory limit.
            memory_swap: self
                .facts()
                .swap_limit
                .then_some(host.memory_swap.unwrap_or(-1)),
            blkio_weight: Some(host.blkio_weight.unwrap_or(500)),
            pids_limit: Some(host.pids_limit.unwrap_or(-1)),
            ..ContainerUpdateBody::default()
        };
        self.daemon
            .docker()
            .update_container(container.id.as_deref().unwrap_or(&unit.reference), update)
            .await
            .map_err(map_error)?;
        Ok(())
    }
}
