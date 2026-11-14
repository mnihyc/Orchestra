//! Effective browser addresses are a committed system-store read, never a service's startup string.

use crate::middleware::requests::ApiResult;
use crate::state::App;
use orch_api_contract::public_addresses::PublicAddressSnapshot;
use orch_db::Conn;

/// Read on the caller's connection, including inside the operation that admits a link or flow.
pub async fn read(app: &App, conn: &mut Conn) -> ApiResult<PublicAddressSnapshot> {
    Ok(orch_db::queries::public_addresses::read(
        conn,
        &app.config.system.address_seed(),
        app.clock.now(),
    )
    .await?)
}

/// Read for a producer that does not already hold a database connection.
pub async fn current(app: &App) -> ApiResult<PublicAddressSnapshot> {
    read(app, &mut *app.db.acquire().await?).await
}

/// A browser origin is one argument in a command copied into a POSIX shell.
#[must_use]
pub fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
