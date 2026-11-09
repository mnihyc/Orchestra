//! The existing one-use host ledger restores the actual original HTTP maker at redemption.

use super::*;

use orch_engine::host_enrollment::Issuer;

use orch_engine::host_enrollment::Redemption;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::http_origin::Current;

use orch_types::Json;
pub(super) fn closed() -> LinkFault {
    LinkFault::new(
        403,
        "enrollment_issuer_closed",
        "The original enrollment issuer no longer permits this host.",
    )
}

async fn packet(conn: &mut orch_db::Conn, hash: &[u8]) -> Result<Option<Issuer>, LinkFault> {
    let saved:Option<(bool,Option<Json>)>=sqlx::query_as("SELECT issuer_required,issuer_origin FROM enrollment_tokens WHERE token_hash=$1 AND purpose='host' AND again_machine_id IS NULL").bind(hash).fetch_optional(conn).await.map_err(DbError::from)?;
    match saved {
        Some((true, None)) => Err(closed()),
        Some((_, Some(value))) => serde_json::from_value(value)
            .map(Some)
            .map_err(|_| closed()),
        _ => Ok(None),
    }
}

pub(super) async fn locks(
    service: &CertificateService,
    conn: &mut orch_db::Conn,
    hash: &[u8],
) -> Result<(), LinkFault> {
    if let Some(packet) = packet(conn, hash).await? {
        let frame = packet
            .prepare(conn, service.inner.clock.as_ref())
            .await
            .map_err(|_| closed())?;
        frame
            .check_deadline(service.inner.clock.as_ref())
            .map_err(|_| closed())?;
    }
    Ok(())
}

pub(super) async fn issuer(
    service: &CertificateService,
    conn: &mut orch_db::Conn,
    row: &TokenRow,
) -> Result<Option<continuing::Frame<Redemption>>, LinkFault> {
    let Some(packet) = packet(conn, &row.token_hash).await? else {
        return Ok(None);
    };
    let frame = packet
        .prepare(conn, service.inner.clock.as_ref())
        .await
        .map_err(|_| closed())?;
    frame
        .check_deadline(service.inner.clock.as_ref())
        .map_err(|_| closed())?;
    Ok(Some(frame))
}

pub(super) async fn deadline(
    service: &CertificateService,
    conn: &mut orch_db::Conn,
    row: &TokenRow,
) -> Result<Current, LinkFault> {
    let packet = packet(conn, &row.token_hash).await?.ok_or_else(closed)?;
    packet
        .current(conn, service.inner.clock.as_ref())
        .await
        .map_err(|_| closed())
}
