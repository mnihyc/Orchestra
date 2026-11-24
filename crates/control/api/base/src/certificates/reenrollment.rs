//! Tokens that enroll an existing machine again and certificate handover without replacing its row or registrations.

use super::*;

use orch_db::queries::resources::reenrollment as rows;

use orch_engine::enrollment_authority::AgainOrigin;

use orch_engine::enrollment_authority::AgainToken;

use orch_pki::enrollment::Enrolled;

use orch_pki::enrollment::IssuedToken;
pub struct Issuer {
    packet: AgainOrigin,
    caller: orch_authz::Caller,
    until: Option<Time>,
}

impl Issuer {
    pub(super) fn check(&self, service: &CertificateService) -> Result<(), LinkFault> {
        if !self
            .caller
            .key_deadline_allows(service.inner.clock.as_ref())
            || self
                .until
                .is_some_and(|until| until.get() <= service.inner.clock.now())
        {
            return Err(issuer_closed());
        }
        Ok(())
    }
    async fn refresh(
        &mut self,
        service: &CertificateService,
        conn: &mut orch_db::Conn,
    ) -> Result<(), LinkFault> {
        let (caller, until) = self
            .packet
            .current(conn, service.inner.clock.as_ref())
            .await
            .map_err(|error| LinkFault::internal(&error))?
            .ok_or_else(issuer_closed)?;
        self.caller = caller;
        self.until = match (self.until, until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.check(service)
    }
}

fn issuer_closed() -> LinkFault {
    LinkFault::new(
        403,
        "enrollment_issuer_closed",
        "The original enrollment issuer no longer permits this replacement.",
    )
}

/// Match the native creator-before-reservation lock order; the locked ledger is checked again below.
pub(super) async fn issuer_locks(
    service: &CertificateService,
    conn: &mut orch_db::Conn,
    hash: &[u8],
) -> Result<(), LinkFault> {
    let Some(value) = resources::replacement_issuer_origin(conn, hash).await? else {
        return Ok(());
    };
    let packet: AgainOrigin = serde_json::from_value(value).map_err(|_| issuer_closed())?;
    if packet
        .current(conn, service.inner.clock.as_ref())
        .await
        .map_err(|error| LinkFault::internal(&error))?
        .is_none()
    {
        return Err(issuer_closed());
    }
    Ok(())
}

/// Only actual stored issuer provenance enters this guard; an absent legacy packet stays legacy.
pub(super) async fn issuer(
    service: &CertificateService,
    conn: &mut orch_db::Conn,
    token: &TokenRow,
) -> Result<Option<Issuer>, LinkFault> {
    let Some(value) = resources::enrollment_issuer_origin(conn, &token.token_hash).await? else {
        return Ok(None);
    };
    let packet: AgainOrigin = serde_json::from_value(value).map_err(|_| issuer_closed())?;
    let binding = AgainToken {
        machine: token.again_machine_id.ok_or_else(issuer_closed)?,
        hash: token.token_hash.clone(),
        issuer: token.created_by,
        purpose: token.purpose.clone(),
        expires: Time::new(token.expires_at),
    };
    if !packet.matches(&binding) {
        return Err(issuer_closed());
    }
    let (caller, until) = packet
        .current(conn, service.inner.clock.as_ref())
        .await
        .map_err(|error| LinkFault::internal(&error))?
        .ok_or_else(issuer_closed)?;
    let expires = Time::new(token.expires_at);
    let issuer = Issuer {
        packet,
        caller,
        until: Some(until.map_or(expires, |until| until.min(expires))),
    };
    issuer.check(service)?;
    Ok(Some(issuer))
}

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
    validated: Enrolled,
    request: &EnrollRequest,
    origin: &Origin,
    issuer: Option<&mut Issuer>,
) -> Result<EnrollReply, LinkFault> {
    let inner = &service.inner;
    let machine = token
        .again_machine_id
        .ok_or_else(|| LinkFault::from(EnrollRefusal::Gone))?;
    let row = rows::lock(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.lost_at.is_none())
        .ok_or_else(|| LinkFault::from(EnrollRefusal::Gone))?;
    if let Some(issuer) = issuer.as_ref() {
        issuer.check(service)?;
    }
    let mut issuer = issuer;
    if let Some(issuer) = issuer.as_mut() {
        issuer.refresh(service, conn).await?;
    }
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
    if let Some(issuer) = issuer.as_ref() {
        issuer.check(service)?;
    }
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
    if let Some(issuer) = issuer.as_ref() {
        issuer.check(service)?;
    }
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
    if let Some(issuer) = issuer.as_ref() {
        issuer.check(service)?;
    }
    let advertisement = inner.advertisement();
    Ok(EnrollReply {
        gateways: advertisement.gateways,
        machine_id: machine,
        certificate_pem: certificate.pem(),
        ca_pem: inner.authority.certificate_pem(),
        endpoints: super::endpoints_for_machine(conn, machine, &advertisement.endpoints).await?,
        system: EnrollReplySystem {
            name: inner.settings.system_name.clone(),
            public_url: orch_db::queries::public_addresses::read(
                conn,
                &inner.settings.address_seed,
                now,
            )
            .await?
            .addresses
            .public_url,
        },
        protocol_version: validated.protocol_version,
    })
}
