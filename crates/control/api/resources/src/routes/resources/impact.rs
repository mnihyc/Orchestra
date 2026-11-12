//! Pause, enable and delete previews read the shared dependency and work service with native resource authority.

use crate::registry::Registry;

use orch_api_contract::resource_facts as api;

use orch_api_contract::resource_facts::FactTarget;

pub use orch_api_base::routes::resources::impact::note;
/// Binds the shared current-consequence read.
pub fn mount(registry: &mut Registry) {
    registry.on_snapshot_body_read_with_optional_operation(
        api::ResourceImpactRead,
        crate::key_operations::native::selected_body::<super::figures::Impact, (), FactTarget>,
        crate::key_operations::native::selected_body_read::<super::figures::Impact, (), FactTarget>,
    );
}
