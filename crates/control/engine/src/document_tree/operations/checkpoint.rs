//! A native writer invokes the original caller's check at its first actual mutation.

use crate::EngineError;
use crate::document_tree::Error;

pub(in crate::document_tree) struct Checkpoint<'a> {
    before: Option<Box<dyn FnOnce() -> Result<(), EngineError> + Send + 'a>>,
}
impl<'a> Checkpoint<'a> {
    pub(in crate::document_tree) fn new<F>(before: F) -> Self
    where
        F: FnOnce() -> Result<(), EngineError> + Send + 'a,
    {
        Self {
            before: Some(Box::new(before)),
        }
    }
    /// Existing native callers have no additional HTTP callback to supply.
    pub(in crate::document_tree) fn native() -> Self {
        Self { before: None }
    }
    pub(in crate::document_tree) fn check_engine(&mut self) -> Result<(), EngineError> {
        match self.before.take() {
            Some(before) => before(),
            None => Ok(()),
        }
    }
    pub(in crate::document_tree) fn check(&mut self) -> Result<(), Error> {
        self.check_engine().map_err(Error::Engine)
    }
}
