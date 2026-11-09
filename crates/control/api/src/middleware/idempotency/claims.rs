//! One lock-only backend per bound application; no transaction or native pool slot spans IO.
use super::*;
use crate::state::App;
use std::sync::Weak;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
/// A contender releases the shared session between short retries so other keys can finish.
const CLAIM_RETRY: std::time::Duration = std::time::Duration::from_millis(100);
type Key = (Id, String);
#[derive(Default)]
pub(crate) struct Claims {
    sessions: Mutex<HashMap<usize, Arc<Session>>>,
}
impl std::fmt::Debug for Claims {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("CreationClaims([opaque])")
    }
}
struct Connection {
    generation: u64,
    value: Option<orch_db::Conn>,
}
struct Session {
    app: App,
    connection: AsyncMutex<Connection>,
    keys: Mutex<HashMap<Key, Weak<AsyncMutex<()>>>>,
}
struct Local {
    session: Arc<Session>,
    key: Key,
    guard: Option<OwnedMutexGuard<()>>,
}
impl Drop for Local {
    fn drop(&mut self) {
        drop(self.guard.take());
        if let Ok(mut keys) = self.session.keys.lock()
            && keys
                .get(&self.key)
                .is_some_and(|slot| slot.strong_count() == 0)
        {
            keys.remove(&self.key);
        }
    }
}
pub(super) struct Lease {
    session: Arc<Session>,
    key: Key,
    generation: u64,
    local: Option<Local>,
}
impl Claims {
    pub(super) async fn lock(&self, app: App, account: Id, key: String) -> ApiResult<Lease> {
        // Retaining the application makes its address stable for the lifetime of this binding.
        let identity = Arc::as_ptr(&app) as usize;
        let session = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| ApiError::internal("creation sessions are poisoned"))?;
            sessions
                .entry(identity)
                .or_insert_with(|| {
                    Arc::new(Session {
                        app,
                        connection: AsyncMutex::new(Connection {
                            generation: 0,
                            value: None,
                        }),
                        keys: Mutex::new(HashMap::new()),
                    })
                })
                .clone()
        };
        let key = (account, key);
        // Never retain this standard MutexGuard over an await.
        let local = {
            let mut keys = session
                .keys
                .lock()
                .map_err(|_| ApiError::internal("creation key slots are poisoned"))?;
            keys.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
                let slot = Arc::new(AsyncMutex::new(()));
                keys.insert(key.clone(), Arc::downgrade(&slot));
                slot
            })
        };
        let local = local.lock_owned().await;
        let local = Local {
            session: session.clone(),
            key: key.clone(),
            guard: Some(local),
        };
        loop {
            if let Some(generation) = session.try_lock(&key).await? {
                return Ok(Lease {
                    session,
                    key,
                    generation,
                    local: Some(local),
                });
            }
            tokio::time::sleep(CLAIM_RETRY).await;
        }
    }
}
impl Session {
    async fn try_lock(&self, key: &Key) -> ApiResult<Option<u64>> {
        let mut connection = self.connection.lock().await;
        if connection.value.is_none() {
            // One extra backend per application, never one per request. Native writers
            // retain the whole transaction pool even while many creation keys wait.
            connection.value = Some(self.app.db.acquire().await?.detach());
            connection.generation = connection.generation.wrapping_add(1);
        }
        let result = orch_db::idempotency::creation::try_lock(
            connection
                .value
                .as_mut()
                .ok_or_else(|| ApiError::internal("the creation lock session disappeared"))?,
            key.0,
            &key.1,
        )
        .await;
        match result {
            Ok(true) => Ok(Some(connection.generation)),
            Ok(false) => Ok(None),
            Err(error) => {
                connection.value = None;
                Err(ApiError::internal(error))
            }
        }
    }
    async fn unlock(&self, key: &Key, generation: u64) {
        let mut connection = self.connection.lock().await;
        if connection.generation != generation {
            return;
        }
        if let Some(value) = connection.value.as_mut() {
            let unlocked = orch_db::idempotency::creation::unlock(value, key.0, &key.1).await;
            if unlocked.is_err() {
                connection.value = None;
            }
        }
    }
}
impl Lease {
    async fn finish(&mut self) {
        self.session.unlock(&self.key, self.generation).await;
        drop(self.local.take());
        if let Ok(mut keys) = self.session.keys.lock()
            && keys
                .get(&self.key)
                .is_some_and(|slot| slot.strong_count() == 0)
        {
            keys.remove(&self.key);
        }
    }
    pub(super) async fn release(mut self) {
        self.finish().await;
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.local.is_none() {
            return;
        }
        let session = self.session.clone();
        let key = self.key.clone();
        let generation = self.generation;
        let local = self.local.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                session.unlock(&key, generation).await;
                drop(local);
                if let Ok(mut keys) = session.keys.lock()
                    && keys.get(&key).is_some_and(|slot| slot.strong_count() == 0)
                {
                    keys.remove(&key);
                }
            });
        }
    }
}
