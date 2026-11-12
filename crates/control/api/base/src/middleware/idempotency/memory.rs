//! An existing native memory store owns its entry until the accepting transaction commits.

use super::*;
/// The real native entry and its owned inverse, never another map, journal or cached response.
/// Dropping a cancelled/failed issuer removes only that entry. A successful issuer publishes once.
#[must_use = "publish the native entry only after its accepting transaction commits"]
pub struct Reservation<T> {
    id: Id,
    value: Option<T>,
    cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl<T> Reservation<T> {
    pub fn new(id: Id, value: T, cancel: impl FnOnce() + Send + 'static) -> Self {
        Self {
            id,
            value: Some(value),
            cancel: Some(Box::new(cancel)),
        }
    }
    pub fn value(&self) -> ApiResult<&T> {
        self.value
            .as_ref()
            .ok_or_else(|| ApiError::internal("the native memory entry was already published"))
    }
    /// Called on the native issuer's actual guarded transaction, before its commit/publication.
    pub async fn record(
        &mut self,
        ctx: &Ctx,
        conn: &mut orch_db::Conn,
        kind: &str,
        facts: Json,
    ) -> ApiResult<()> {
        rows::creation::observe_with(conn, Some(ctx.user()?.id()), kind, self.id, facts).await?;
        Ok(())
    }
    /// Only the native issuer's successful transaction can expose its once-only capability.
    pub fn publish(mut self) -> ApiResult<T> {
        let value = self
            .value
            .take()
            .ok_or_else(|| ApiError::internal("the native memory entry was already published"))?;
        self.cancel = None;
        Ok(value)
    }
}

impl<T> Drop for Reservation<T> {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn only_publishing_keeps_the_native_memory_entry() {
        for publish in [false, true] {
            let entries = Arc::new(Mutex::new(HashMap::new()));
            let id = new_id();
            entries.lock().unwrap().insert(id, "native entry");
            let owned = entries.clone();
            let reservation = Reservation::new(id, "first-only", move || {
                owned.lock().unwrap().remove(&id);
            });
            if publish {
                assert_eq!(reservation.publish().unwrap(), "first-only");
            } else {
                drop(reservation);
            }
            assert_eq!(entries.lock().unwrap().contains_key(&id), publish);
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_only_publishing_keeps_the_native_memory_entry() {
        self::only_publishing_keeps_the_native_memory_entry()
    }
}
