//! Native job identities replay through the one creation seam and current completed-read factory.

use super::*;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment_sources::job_creation as sources;

use orch_api_contract::environment_sources::job_creation::Source;

use orch_api_contract::operation_sources::Declaration;

pub use orch_api_base::routes::environments::jobs::creation::Mode;

pub use orch_api_base::routes::environments::jobs::creation::ProviderUpdate;

pub use orch_api_base::routes::environments::jobs::creation::binding;

pub use orch_api_base::routes::environments::jobs::creation::replay;

pub use orch_api_base::routes::environments::jobs::creation::Input;

pub use orch_api_base::routes::environments::jobs::creation::Content;

pub use orch_api_base::routes::environments::jobs::creation::respond;

pub use orch_api_base::routes::environments::jobs::creation::observed;
pub(crate) struct EnvironmentUpdate;

impl Mode for EnvironmentUpdate {
    const KIND: &'static str = "environment_update_job";
    const OBJECT: ObjectKind = ObjectKind::Environment;
    const ROW: &'static Declaration<Source> = &sources::ENVIRONMENT_UPDATE;
    fn governor(row: &JobRow) -> Option<Id> {
        row.environment_id
    }
}
