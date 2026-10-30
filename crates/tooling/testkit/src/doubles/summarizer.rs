//! Rule-based summarizer.
//!
//! Requests are retained and answered in order, so a test can hold a person's competing edit in flight.

use async_trait::async_trait;
use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::result::SummaryResult;
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

/// A producer with explicit, ordered answers and observable requests.
#[derive(Default)]
pub struct Scripted {
    requests: Mutex<Vec<SummaryRequest>>,
    answers: Mutex<VecDeque<Result<SummaryResult, SummarizeError>>>,
}

impl Scripted {
    /// Supplies the next answer.
    pub fn push(&self, answer: Result<SummaryResult, SummarizeError>) {
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(answer);
    }
    /// All requests the producer actually saw.
    #[must_use]
    pub fn requests(&self) -> Vec<SummaryRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Summarizer for Scripted {
    fn describe(&self) -> SummarizerDescriptor {
        SummarizerDescriptor {
            name: "scripted".to_owned(),
            title: "Scripted summaries".to_owned(),
            config_schema: json!({"type":"object"}),
            fields: SummaryField::ALL.to_vec(),
            extensions: Default::default(),
        }
    }
    async fn summarize(&self, request: SummaryRequest) -> Result<SummaryResult, SummarizeError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| {
                Err(SummarizeError::Failed {
                    message: "no scripted answer was supplied".to_owned(),
                    retryable: false,
                })
            })
    }
}
