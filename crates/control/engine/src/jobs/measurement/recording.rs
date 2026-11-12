//! Native review metadata records the references its real planner observed, without retaining a credential.
use super::{Count, EngineError, Request};
use orch_api_contract::forks::Measurement;
use orch_types::Hash;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

/// Data references from this native planner, never an operation or permission proof.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SavedMeasurements(BTreeMap<String, Measurement>);
impl SavedMeasurements {
    pub(crate) fn reference(&self, request: &Request) -> Result<Option<Measurement>, EngineError> {
        Ok(self.0.get(&Self::identity(request)?.to_string()).copied())
    }
    pub(crate) fn identity(request: &Request) -> Result<Hash, EngineError> {
        request.hash()
    }
}
tokio::task_local! {static RECORD:Arc<Mutex<SavedMeasurements>>;}
/// Record only actual returned native counts. This does not change claims, ownership, clocks or IO.
pub async fn record<T>(future: impl std::future::Future<Output = T>) -> (T, SavedMeasurements) {
    let saved = RECORD
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::new(Mutex::new(SavedMeasurements::default())));
    let output = RECORD.scope(Arc::clone(&saved), future).await;
    let observations = saved.lock().unwrap_or_else(PoisonError::into_inner).clone();
    (output, observations)
}
pub(super) fn observe(request: &Request, count: &Count) -> Result<(), EngineError> {
    if let Ok(saved) = RECORD.try_with(Arc::clone) {
        saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .insert(
                SavedMeasurements::identity(request)?.to_string(),
                count.reference,
            );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::measurement::Count;
    use orch_link_contract::commands::CommandKind;
    use orch_types::new_id;
    use serde_json::json;
    #[tokio::test]
    async fn a_native_review_records_only_actual_data_references_and_binds_the_exact_request() {
        let request = Request {
            machine: new_id(),
            kind: CommandKind::NodeResourcesList,
            body: json!({}),
            subject: json!({"host_id":new_id()}),
            by: new_id(),
            reuse_unsupported: false,
        };
        let reference = Measurement {
            job_id: new_id(),
            measured_at: None,
        };
        let count = Count {
            reference,
            result: None,
            unsupported: false,
            origin: None,
        };
        let (observed, saved) = record(async { observe(&request, &count) }).await;
        assert!(observed.is_ok());
        assert_eq!(saved.reference(&request).unwrap_or(None), Some(reference));
        let mut different = request.clone();
        different.machine = new_id();
        assert_eq!(saved.reference(&different).unwrap_or(None), None);
        different = request.clone();
        different.body = json!({"changed":true});
        assert_eq!(saved.reference(&different).unwrap_or(None), None);
        let serialized = serde_json::to_value(&saved).unwrap_or(serde_json::Value::Null);
        assert!(!serialized.to_string().contains("origin"));
        assert!(!serialized.to_string().contains("credential"));
    }
}
