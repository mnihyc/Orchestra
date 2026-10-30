//! Tokens that enroll an existing machine again and certificate handover without replacing its row or registrations.

use super::*;
use orch_db::queries::resources::reenrollment as rows;
use orch_pki::enrollment::{Enrolled, IssuedToken};

pub(super) async fn token(
    service: &CertificateService,
    request: &TokenRequest,
    machine: Id,
    issued: &IssuedToken,
) -> Result<NewToken, LinkFault> {
    let mut conn = service.inner.db.acquire().await?;
    let row = resources::machine(&mut conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.lost_at.is_none())
        .ok_or_else(|| LinkFault::new(404, "not_found", "the machine is not there"))?;
    let host = rows::host_of(&mut conn, machine).await?.is_some();
    resources::insert_enrollment_token(
        &mut conn,
        &TokenRow {
            token_hash: raw_hash(&issued.digest),
            created_by: request.created_by,
            purpose: if host { "host" } else { "connected" }.to_owned(),
            org_id: None,
            name_hint: Some(row.name),
            expires_at: utc(issued.expires_at),
            used_at: None,
            machine_id: None,
            again_machine_id: Some(machine),
        },
    )
    .await?;
    Ok(NewToken {
        token: issued.token.to_string(),
        expires_at: issued.expires_at,
        receipt: issued.digest.clone(),
    })
}

pub(super) async fn finish(
    service: &CertificateService,
    conn: &mut orch_db::Conn,
    token: &TokenRow,
    machine: Id,
    validated: Enrolled,
    request: &EnrollRequest,
    origin: &Origin,
) -> Result<EnrollReply, LinkFault> {
    let inner = &service.inner;
    let row = rows::lock(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.lost_at.is_none())
        .ok_or_else(|| LinkFault::from(EnrollRefusal::Gone))?;
    // The ordinary enrollment validator has checked the token, pin, protocol, roles and CSR.
    // The token alone chooses the retained identity; the CSR's subject never does.
    let csr = CertificateRequest::from_pem(&request.csr_pem)
        .map_err(|_| LinkFault::from(EnrollRefusal::Invalid))?;
    let certificate = inner.authority.issue(
        &csr,
        &orch_pki::ca::Holder::Machine(machine),
        inner.settings.leaf_validity_days,
        inner.clock.as_ref(),
    )?;
    let now = utc(inner.clock.time());
    rows::activate(
        conn,
        &row,
        &NextCertificate {
            fingerprint: certificate.fingerprint(),
            public_key: RecipientKey::from_certificate(&certificate).to_text(),
            serial: hex::encode(certificate.serial()),
            not_after: utc(certificate.not_after()),
            again: true,
        },
        now,
    )
    .await?;
    if !resources::spend_enrollment_token(conn, &token.token_hash, machine, now).await? {
        return Err(EnrollRefusal::Gone.into());
    }
    let summary = format!("Machine {} enrolled again", row.name);
    let mut entry = Entry::new(
        Actor::User(token.created_by),
        "machine.enrolled",
        "machine",
        Some(machine),
        &summary,
    );
    entry.details = serde_json::json!({"again": true, "serial": hex::encode(certificate.serial())});
    audit::append(conn, &entry, origin, now).await?;
    Ok(EnrollReply {
        machine_id: machine,
        certificate_pem: certificate.pem(),
        ca_pem: inner.authority.certificate_pem(),
        endpoints: super::endpoints_for_machine(conn, machine, &inner.settings.endpoints).await?,
        system: EnrollReplySystem {
            name: inner.settings.system_name.clone(),
            public_url: inner.settings.public_url.clone(),
        },
        protocol_version: validated.protocol_version,
    })
}
