//! Same-transaction native replacement reservations and their private original issuer packet.

use super::*;

use orch_engine::enrollment_authority::AgainOrigin;

use orch_engine::enrollment_authority::AgainToken;
pub struct PreparedAgainToken {
    issued: IssuedToken,
    row: TokenRow,
    machine: Id,
    transaction: String,
}

impl PreparedAgainToken {
    pub fn binding(&self) -> AgainToken {
        AgainToken {
            machine: self.machine,
            hash: self.row.token_hash.clone(),
            issuer: self.row.created_by,
            purpose: self.row.purpose.clone(),
            expires: Time::new(self.row.expires_at),
        }
    }
}

impl CertificateService {
    /// Make the existing native one-use token without a ledger write or HTTP authority claim.
    pub async fn prepare_again_token(
        &self,
        conn: &mut orch_db::Conn,
        issuer: Id,
        machine: Id,
    ) -> Result<PreparedAgainToken, LinkFault> {
        let row = resources::machine(conn, machine)
            .await?
            .filter(|row| row.deleted_at.is_none() && row.lost_at.is_none())
            .ok_or_else(|| LinkFault::new(404, "not_found", "the machine is not there"))?;
        let host = resources::reenrollment::host_of(conn, machine)
            .await?
            .is_some();
        let issued = self.fresh_token();
        let transaction = host_token_transaction(conn).await?;
        Ok(PreparedAgainToken {
            row: TokenRow {
                token_hash: raw_hash(&issued.digest),
                created_by: issuer,
                purpose: if host { "host" } else { "connected" }.into(),
                org_id: None,
                name_hint: Some(row.name),
                expires_at: utc(issued.expires_at),
                used_at: None,
                machine_id: None,
                again_machine_id: Some(machine),
            },
            machine,
            issued,
            transaction,
        })
    }
    /// Consume this same native reservation at its first INSERT, atomically retaining its issuer.
    pub async fn apply_again_token<E, F>(
        &self,
        conn: &mut orch_db::Conn,
        prepared: PreparedAgainToken,
        issuer: &AgainOrigin,
        before: F,
    ) -> Result<Result<NewToken, LinkFault>, E>
    where
        F: FnOnce() -> Result<(), E>,
    {
        let valid:Result<(),LinkFault>=async {
            if !orch_db::queries::transaction_guard::in_transaction(conn) || host_token_transaction(conn).await?!=prepared.transaction
                || !prepared.issued.token.pins(&self.inner.authority.pin()) || !issuer.matches(&prepared.binding()) {
                return Err(LinkFault::invalid("the replacement reservation belongs to another transaction, authority or issuer"));
            }
            Ok(())
        }.await;
        if let Err(error) = valid {
            return Ok(Err(error));
        }
        before()?;
        Ok(async {
            if !resources::insert_enrollment_token(conn, &prepared.row).await? {
                return Err(LinkFault::internal(
                    &"could not reserve the replacement token",
                ));
            }
            let value = serde_json::to_value(issuer)
                .map_err(|_| LinkFault::internal(&"could not retain the replacement issuer"))?;
            if !resources::set_enrollment_issuer_origin(conn, &prepared.row.token_hash, &value)
                .await?
            {
                return Err(LinkFault::internal(
                    &"could not retain the replacement issuer",
                ));
            }
            Ok(NewToken {
                token: prepared.issued.token.to_string(),
                expires_at: prepared.issued.expires_at,
                receipt: prepared.issued.digest,
            })
        }
        .await)
    }
}
